//! `read-local-file` — the first real bunzo skill.
//!
//! Receives `{ "path": "..." }` as JSON input, asks the host to read the file
//! (the host enforces the manifest's path whitelist), and returns the
//! contents. Designed to be the smallest possible demonstration of
//! capability-scoped skill execution.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use bunzo_skill_abi::{bunzo_skill, fs_read};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Input {
    path: String,
}

#[derive(Serialize)]
struct Output {
    path: String,
    content: String,
}

bunzo_skill!(run_skill);

fn run_skill(input: Input) -> Result<Output, &'static str> {
    let bytes = fs_read(&input.path)?;
    let content = core::str::from_utf8(&bytes)
        .map_err(|_| "file is not valid UTF-8")?
        .to_string();
    Ok(Output {
        path: input.path,
        content,
    })
}
