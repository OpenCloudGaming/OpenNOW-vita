.PHONY: vpk desktop ftp eboot upload-vpk update-run-vita run-vita

RUSTFLAGS ?= -C target-feature=-neon
CARGO_VITA ?= cargo +nightly vita
VPK := target/armv7-sony-vita-newlibeabihf/release/opennow-vita.vpk
VITA_UPLOAD_DIR ?= ux0:/data/
DESKTOP_DIR ?= $(HOME)/Desktop
VPK_NAME := opennow-vita.vpk
FTP_PORT ?= 1337

vpk:
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO_VITA) build vpk --release

# Release build dropped straight on the Desktop for manual install via VitaShell/USB.
# Preferred over `upload-vpk`: `cargo vita upload` connects and then exits without transferring
# anything, and FTPVita's data channel often drops mid-transfer on a weak link.
desktop: vpk
	cp $(VPK) $(DESKTOP_DIR)/opennow-vita.vpk
	@ls -lh $(DESKTOP_DIR)/opennow-vita.vpk

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
	@local_size=$$(wc -c < $(VPK) | tr -d ' '); \
	remote_url="ftp://$(VITA_IP):$(FTP_PORT)/$(VITA_UPLOAD_DIR)$(VPK_NAME)"; \
	remote_size=$$(curl -sS -I --connect-timeout 15 --max-time 60 "$$remote_url" 2>/dev/null \
		| tr -d '\r' | awk -F': ' '/[Cc]ontent-[Ll]ength/ {print $$2}'); \
	if [ "$(FORCE)" != "1" ] && [ -n "$$remote_size" ] && [ "$$remote_size" = "$$local_size" ]; then \
		echo "Vita already has this exact build ($$local_size bytes) - skipping the upload."; \
		echo "Re-send it anyway with: make ftp VITA_IP=$(VITA_IP) FORCE=1"; \
		exit 0; \
	fi; \
	echo "Uploading $$(du -h $(VPK) | cut -f1) to $(VITA_IP)..."; \
	curl -S --progress-bar --connect-timeout 15 --max-time 900 -T $(VPK) "$$remote_url" \
		-w "transfer: %{size_upload} bytes in %{time_total}s (%{speed_upload} B/s)\n"; \
	echo "local:  $$local_size bytes"; \
	printf "remote: "; \
	curl -sS -I --connect-timeout 15 --max-time 60 "$$remote_url" \
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
