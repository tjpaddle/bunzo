//! wasmtime embedding — compiles each skill's WASM once at bunzod startup,
//! and instantiates a fresh `Store` per invocation so leaked skill memory and
//! allocator state never cross request boundaries.
//!
//! The host API given to skills is deliberately narrow: `bunzo_fs_read`
//! (capability-checked) and `bunzo_log` (unprivileged stderr echo). Anything
//! else is out of reach by construction — that is the sandbox.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use wasmtime::{
    Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};

use super::manifest::{Capabilities, Manifest};

/// Max memory a single skill invocation is allowed to allocate via
/// `memory.grow` (the WASM linear memory), on top of whatever the module
/// already declares as its initial pages.
const MAX_MEMORY_BYTES: usize = 32 * 1024 * 1024;

/// Wasmtime fuel budget per invocation. 10M is generous for a chat-shaped
/// skill; prevents a pathological module from looping forever.
const FUEL_BUDGET: u64 = 10_000_000;

pub struct Skill {
    pub manifest: Manifest,
    module: Module,
}

pub struct SkillHost {
    engine: Engine,
}

impl SkillHost {
    pub fn new() -> Result<Self> {
        let mut cfg = Config::new();
        cfg.consume_fuel(true);
        let engine = Engine::new(&cfg).context("building wasmtime engine")?;
        Ok(Self { engine })
    }

    pub fn compile(&self, wasm_path: &PathBuf) -> Result<Module> {
        let bytes =
            std::fs::read(wasm_path).with_context(|| format!("reading {}", wasm_path.display()))?;
        Module::new(&self.engine, &bytes)
            .with_context(|| format!("compiling {}", wasm_path.display()))
    }

    pub fn build_skill(&self, manifest: Manifest, wasm_path: &PathBuf) -> Result<Skill> {
        let module = self.compile(wasm_path)?;
        Ok(Skill { manifest, module })
    }

    /// Invoke a skill with a JSON argument. Returns the skill's JSON output.
    /// All skill-side errors surface as `Err`; policy denials also return
    /// `Err`.
    pub fn invoke(&self, skill: &Skill, args_json: &str) -> Result<String> {
        let mut store = Store::new(
            &self.engine,
            StoreState {
                caps: skill.manifest.capabilities.clone(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(MAX_MEMORY_BYTES)
                    .build(),
                skill_name: skill.manifest.name.clone(),
            },
        );
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(FUEL_BUDGET)
            .context("setting wasmtime fuel")?;

        let mut linker: Linker<StoreState> = Linker::new(&self.engine);
        linker
            .func_wrap("bunzo", "bunzo_fs_read", host_fs_read)
            .context("registering bunzo_fs_read")?;
        linker
            .func_wrap("bunzo", "bunzo_log", host_log)
            .context("registering bunzo_log")?;

        let instance = linker
            .instantiate(&mut store, &skill.module)
            .context("instantiating skill module")?;

        // Stage the input buffer inside the guest.
        let alloc = guest_alloc(&instance, &mut store)?;
        let memory = guest_memory(&instance, &mut store)?;

        let input_bytes = args_json.as_bytes();
        let input_ptr = alloc
            .call(&mut store, input_bytes.len() as u32)
            .context("calling bunzo_alloc for input")?;
        memory
            .write(&mut store, input_ptr as usize, input_bytes)
            .context("writing input to guest memory")?;

        // Call `run`.
        let run: TypedFunc<(u32, u32), u64> = instance
            .get_typed_func(&mut store, "run")
            .context("skill missing `run` export")?;
        let packed = run
            .call(&mut store, (input_ptr, input_bytes.len() as u32))
            .context("invoking skill run")?;

        if packed == 0 {
            bail!("skill returned error");
        }
        let out_ptr = (packed >> 32) as u32;
        let out_len = packed as u32;

        let mut out = vec![0u8; out_len as usize];
        memory
            .read(&mut store, out_ptr as usize, &mut out)
            .context("reading skill output")?;
        let out_str = String::from_utf8(out).context("skill output not UTF-8")?;
        Ok(out_str)
    }
}

fn guest_alloc(instance: &Instance, store: &mut Store<StoreState>) -> Result<TypedFunc<u32, u32>> {
    instance
        .get_typed_func::<u32, u32>(store, "bunzo_alloc")
        .context("skill missing `bunzo_alloc` export")
}

fn guest_memory(instance: &Instance, store: &mut Store<StoreState>) -> Result<Memory> {
    instance
        .get_memory(store, "memory")
        .ok_or_else(|| anyhow!("skill missing `memory` export"))
}

pub struct StoreState {
    caps: Capabilities,
    limits: StoreLimits,
    skill_name: String,
}

fn host_fs_read(mut caller: Caller<'_, StoreState>, path_ptr: u32, path_len: u32) -> u64 {
    // Pull the guest memory handle; if the skill somehow doesn't export
    // memory, there's nothing we can do with ptr/len, so treat as failure.
    let memory = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return 0,
    };

    let mut path_buf = vec![0u8; path_len as usize];
    if memory
        .read(&mut caller, path_ptr as usize, &mut path_buf)
        .is_err()
    {
        return 0;
    }
    let path = match std::str::from_utf8(&path_buf) {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };

    let skill_name = caller.data().skill_name.clone();
    if !caller.data().caps.allows_read(&path) {
        eprintln!(
            "bunzod: [skill:{skill_name}] fs_read DENIED: {path} (not in manifest capabilities)",
        );
        return 0;
    }

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bunzod: [skill:{skill_name}] fs_read failed: {path}: {e}");
            return 0;
        }
    };

    // Ask the guest to allocate an output buffer and write into it.
    let alloc = match caller.get_export("bunzo_alloc") {
        Some(Extern::Func(f)) => match f.typed::<u32, u32>(&caller) {
            Ok(t) => t,
            Err(_) => return 0,
        },
        _ => return 0,
    };
    let out_ptr = match alloc.call(&mut caller, bytes.len() as u32) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    if memory.write(&mut caller, out_ptr as usize, &bytes).is_err() {
        return 0;
    }
    ((out_ptr as u64) << 32) | (bytes.len() as u64)
}

fn host_log(mut caller: Caller<'_, StoreState>, ptr: u32, len: u32) {
    let memory = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return,
    };
    let mut buf = vec![0u8; len as usize];
    if memory.read(&mut caller, ptr as usize, &mut buf).is_err() {
        return;
    }
    let skill_name = caller.data().skill_name.clone();
    let msg = String::from_utf8_lossy(&buf);
    eprintln!("bunzod: [skill:{skill_name}] {msg}");
}
