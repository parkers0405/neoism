.DEFAULT_GOAL := build
.PHONY: build run dev install check test docs docs-init docs-check
docs-init:
	git submodule update --init --recursive vendor/neoism-docs
docs:
	node scripts/build-docs.mjs
docs-check:
	node --test scripts/build-docs.test.mjs
build:
	bash scripts/gpuix.sh build
run:
	bash scripts/gpuix.sh run
dev:
	bash scripts/gpuix.sh dev
install:
	./install.sh
check:
	bash scripts/gpuix.sh check
test:
	bash scripts/gpuix.sh test
