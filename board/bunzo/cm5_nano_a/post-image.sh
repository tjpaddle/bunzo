#!/usr/bin/env bash
#
# post-image.sh for bunzo CM5-NANO-A target.
#
# Runs after Buildroot finishes assembling the kernel and rootfs. Produces
# output/images/sdcard.img via genimage. The FAT boot partition's contents
# are discovered at image time by listing rpi-firmware/* and *.dtb under
# BINARIES_DIR, then substituting into genimage.cfg.in. Matches the
# pattern used by Buildroot's upstream board/raspberrypi/post-image.sh.
#
# Flash the resulting sdcard.img onto the CM5 eMMC via rpiboot over
# USB-C on the Waveshare CM5-NANO-A (hold BOOT, connect USB-C to host,
# power on, run rpiboot, then dd the image to the enumerated block
# device).

set -e
set -u

BOARD_DIR="$(dirname "$0")"
GENIMAGE_TEMPLATE="${BOARD_DIR}/genimage.cfg.in"
GENIMAGE_CFG="${BINARIES_DIR}/genimage.cfg"
GENIMAGE_TMP="${BUILD_DIR}/genimage.tmp"

FILES=()
for i in "${BINARIES_DIR}"/*.dtb "${BINARIES_DIR}"/rpi-firmware/*; do
	[[ -e "${i}" ]] || continue
	FILES+=( "${i#${BINARIES_DIR}/}" )
done

KERNEL=$(sed -n 's/^kernel=//p' "${BINARIES_DIR}/rpi-firmware/config.txt")
FILES+=( "${KERNEL}" )

BOOT_FILES=$(printf '\\t\\t\\t"%s",\\n' "${FILES[@]}")
sed "s|#BOOT_FILES#|${BOOT_FILES}|" "${GENIMAGE_TEMPLATE}" > "${GENIMAGE_CFG}"

trap 'rm -rf "${ROOTPATH_TMP}"' EXIT
ROOTPATH_TMP="$(mktemp -d)"

rm -rf "${GENIMAGE_TMP}"

genimage \
	--rootpath "${ROOTPATH_TMP}" \
	--tmppath "${GENIMAGE_TMP}" \
	--inputpath "${BINARIES_DIR}" \
	--outputpath "${BINARIES_DIR}" \
	--config "${GENIMAGE_CFG}"
