ifeq ($(RELEASE), 1)
CARGO_FLAGS = --release
CARGO_PROFILE = release
else
CARGO_PROFILE = debug
endif

all: zeta plugins
	@echo Build complete.

plugins: zeta
	cd ./plugins && cargo build $(CARGO_FLAGS) && \
	  install -m755 target/$(CARGO_PROFILE)/libzeta_plugins.so ../target/debug

zeta:
	cargo build $(CARGO_FLAGS)

.PHONY: zeta
