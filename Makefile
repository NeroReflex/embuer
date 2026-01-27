# Makefile for Embuer

CC?=cc

.PHONY: all build build-release clean test install install-header example

# Default target
all: build

# Generate C header from Rust source
header:
	cargo build --release
	@find target -name embuer.h -exec cp {} . \; 2>/dev/null || true

# Build in debug mode
build:
	cargo build

# Build in release mode
build-release:
	cargo build --release
	@find target -name embuer.h -exec cp {} . \; 2>/dev/null || true

# Clean build artifacts
clean:
	cargo clean
	rm -rf target/examples

# Run tests
test:
	cargo test

# Build the C example
example: build-release
	mkdir -p target/examples
	$(CC) -O2 \
		-o target/examples/embuer_example examples/embuer_example.c \
		-I. \
		-L./target/release \
		-lembuer \
		-lpthread -ldl -lm

# Build the status monitor example
status-monitor: build-release
	mkdir -p target/examples
	$(CC) -O2 \
		-o target/examples/status_monitor examples/status_monitor.c \
		-I. \
		-L./target/release \
		-lembuer \
		-lpthread -ldl -lm

# Build all examples
examples: example status-monitor

# Install the library and header (requires root)
install: build-release install-header
	mkdir -p $(DESTDIR)/usr/bin/
	install -m 755 target/release/embuer-service $(DESTDIR)/usr/bin/
	install -m 755 target/release/embuer-client $(DESTDIR)/usr/bin/
	install -m 755 target/release/embuer-installer $(DESTDIR)/usr/bin/
	mkdir -p $(DESTDIR)/usr/lib/
	install -m 644 target/release/libembuer.so $(DESTDIR)/usr/lib/
	install -m 644 target/release/libembuer.a $(DESTDIR)/usr/lib/
	mkdir -p $(DESTDIR)/usr/lib/systemd/system/
	install -m 644 rootfs/usr/lib/systemd/system/embuer.service $(DESTDIR)/usr/lib/systemd/system/
	mkdir -p $(DESTDIR)/usr/share/dbus-1/system.d/
	install -m 644 rootfs/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf $(DESTDIR)/usr/share/dbus-1/system.d/
#	ldconfig

# Install just the header file
install-header:
	mkdir -p $(DESTDIR)/usr/include/
	install -m 644 embuer.h $(DESTDIR)/usr/include/

# Run the service (requires root)
run-service: build-release
	sudo ./target/release/embuer-service

# Run the example C program
run-example: example
	LD_LIBRARY_PATH=./target/release ./target/examples/embuer_example

# Run the status monitor
run-monitor: status-monitor
	LD_LIBRARY_PATH=./target/release ./target/examples/status_monitor

help:
	@echo "Embuer Makefile"
	@echo ""
	@echo "Targets:"
	@echo "  all           - Build in debug mode (default)"
	@echo "  build         - Build in debug mode"
	@echo "  build-release - Build in release mode"
	@echo "  clean         - Remove build artifacts"
	@echo "  test          - Run tests"
	@echo "  example       - Build the C example program"
	@echo "  status-monitor- Build the status monitor example"
	@echo "  examples      - Build all C examples"
	@echo "  install       - Install binaries and libraries (requires root)"
	@echo "  install-header- Install only the header file"
	@echo "  run-service   - Build and run the service (requires root)"
	@echo "  run-example   - Build and run the C example"
	@echo "  run-monitor   - Build and run the status monitor"
	@echo "  help          - Show this help"

