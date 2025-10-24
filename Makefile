# Makefile for Embuer

CC?=cc

.PHONY: all build build-release clean test install install-header example

# Default target
all: build

# Build in debug mode
build:
	cargo build

# Build in release mode
build-release:
	cargo build --release

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
	install -m 755 target/release/embuer-service $(DESTDIR)/usr/local/bin/
	install -m 755 target/release/embuer-client $(DESTDIR)/usr/local/bin/
	install -m 644 target/release/libembuer.so $(DESTDIR)/usr/local/lib/
	install -m 644 target/release/libembuer.a $(DESTDIR)/usr/local/lib/
	install -m rootfs/usr/local/lib/systemd/system/embuer.service $(DESTDIR)/usr/local/lib/systemd/system/
	ldconfig

# Install just the header file
install-header:
	install -m 644 embuer.h /usr/local/include/

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

