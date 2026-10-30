.DEFAULT_GOAL := build
.PHONY: build run dev install check test
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
