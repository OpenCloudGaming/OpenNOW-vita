.PHONY: vpk desktop ftp eboot upload-vpk update-run-vita run-vita

RUSTFLAGS ?= -C target-feature=-neon
CARGO_VITA ?= cargo +nightly vita
VPK := target/armv7-sony-vita-newlibeabihf/release/jade-vita.vpk
VITA_UPLOAD_DIR ?= ux0:/data/
DESKTOP_DIR ?= $(HOME)/Desktop
VPK_NAME := jade-vita.vpk
FTP_PORT ?= 1337

vpk:
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO_VITA) build vpk --release

# Release build dropped straight on the Desktop for manual install via VitaShell/USB.
# Preferred over `upload-vpk`: `cargo vita upload` connects and then exits without transferring
# anything, and FTPVita's data channel often drops mid-transfer on a weak link.
desktop: vpk
	cp $(VPK) $(DESKTOP_DIR)/jade-vita.vpk
	@ls -lh $(DESKTOP_DIR)/jade-vita.vpk

# Upload the release VPK to the Vita over VitaShell's FTP (SELECT to start it).
#
# Uses curl rather than `cargo vita upload` (see `upload-vpk`), which connects, prints
# "Uploading..." and then exits without transferring a single byte. Afterwards it re-queries the
# file with SIZE and compares against the local length, because FTPVita's directory listing does
# not refresh after a write - the listing is not proof the upload landed.
ftp: vpk
ifndef VITA_IP
	$(error Usage: make ftp VITA_IP=192.168.0.108)
endif
	curl -sS --connect-timeout 15 --max-time 900 -T $(VPK) \
		"ftp://$(VITA_IP):$(FTP_PORT)/$(VITA_UPLOAD_DIR)$(VPK_NAME)" \
		-w "uploaded %{size_upload} bytes in %{time_total}s (%{speed_upload} B/s)\n"
	@echo "local:  $$(wc -c < $(VPK)) bytes"
	@printf "remote: "; curl -sS -I --connect-timeout 15 --max-time 60 \
		"ftp://$(VITA_IP):$(FTP_PORT)/$(VITA_UPLOAD_DIR)$(VPK_NAME)" \
		| tr -d '\r' | awk -F': ' '/[Cc]ontent-[Ll]ength/ {print $$2 " bytes"}'
	@echo "Now install ux0:/data/$(VPK_NAME) from VitaShell - copying the file does not install it."

eboot:
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO_VITA) build eboot --release

upload-vpk: vpk
ifndef VITA_IP
	$(error Usage: make upload-vpk VITA_IP=192.168.0.103)
endif
	$(CARGO_VITA) upload --vita-ip $(VITA_IP) --source $(VPK) --destination $(VITA_UPLOAD_DIR)

update-run-vita:
ifndef VITA_IP
	$(error Usage: make update-run-vita VITA_IP=192.168.0.103)
endif
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO_VITA) build eboot --update --run --vita-ip $(VITA_IP) -- --release

run-vita: update-run-vita
