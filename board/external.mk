# bunzo external Buildroot tree.
#
# Includes every custom package makefile under board/bunzo/package/.
# No custom packages yet in M1 — the glob is harmless when empty.

include $(sort $(wildcard $(BR2_EXTERNAL_BUNZO_PATH)/bunzo/package/*/*.mk))
