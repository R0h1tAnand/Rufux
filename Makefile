CARGO ?= cargo

.PHONY: all build release test check clean run

all: release

build:
	$(CARGO) build

release:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --locked

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked

run:
	$(CARGO) run --release

clean:
	$(CARGO) clean
