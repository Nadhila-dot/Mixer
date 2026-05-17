.PHONY: install dev build release clean

## Install frontend deps (run once after clone)
install:
	cd frontend && bun install

## ── Dev ──────────────────────────────────────────────────────────────────────
##
## One command: `cargo run` (debug build) auto-detects dev mode and spawns
## `bun run dev` internally — identical to how `php artisan serve` starts Vite.
##
##   http://localhost:3000  →  Rust (SSR data + API)
##   http://localhost:5173  →  Vite (HMR, live reload)
##
## Ctrl-C shuts down both processes together (same process group / SIGINT).
##
dev:
	cargo run

## ── Production ───────────────────────────────────────────────────────────────

## Build frontend assets only (generates frontend/dist)
build-fe:
	cd frontend && bun run build

## Full dev build (frontend + Rust, unoptimised)
build: build-fe
	cargo build

## Optimised release binary — everything embedded into one file
release: build-fe
	cargo build --release
	@echo ""
	@printf "  ✓ Binary: \033[1mtarget/release/forge\033[0m  (%s)\n" \
		"$$(ls -lh target/release/forge | awk '{print $$5}')"

## ── Cleanup ──────────────────────────────────────────────────────────────────

clean:
	rm -rf frontend/dist frontend/node_modules
	cargo clean
