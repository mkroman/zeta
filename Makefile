ifeq ($(RELEASE), 1)
CARGO_FLAGS = --release
CARGO_PROFILE = release
else
CARGO_PROFILE = debug
endif

all: zeta plugins
	@echo Build complete.

plugins: zeta
	cd ./plugins && cargo rustc -Cprefer-dynamic $(CARGO_FLAGS) && \
	  install -m755 target/$(CARGO_PROFILE)/libzeta_plugins.so ../target/$(CARGO_PROFILE)

zeta:
	cargo build $(CARGO_FLAGS)

.PHONY: zeta
