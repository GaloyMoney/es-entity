# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Development Commands

### Database Setup
- `make start-deps` - Start PostgreSQL in Docker
- `make setup-db` - Run database migrations
- `make reset-deps` - Clean, start, and setup database

### Testing
- `cargo test` - Run all tests
- `cargo nextest run` - Run tests with nextest (preferred in CI)
- `cargo test --doc` - Run documentation tests
- Run a single test: `cargo test test_name`

### Code Quality
- `cargo fmt` - Format code
- `cargo fmt --check --all` - Check formatting
- `cargo clippy --workspace` - Run linter
- `cargo check` - Type check
- `cargo audit` - Security audit
- `cargo deny check` - Check dependencies

### Build
- `cargo build` - Build debug
- `cargo build --release` - Build release
- `cargo doc --no-deps` - Generate documentation

### Database Migrations
- `cargo sqlx migrate run` - Run migrations
- `cargo sqlx prepare` - Generate offline query data

## Architecture Overview

This is an Event Sourcing Entity Framework for Rust that provides:

1. **Core Traits** (`src/traits.rs`):
   - `EsEvent`: Events that represent state changes
   - `EsEntity`: Entities built from event streams
   - `EsRepo`: Repository pattern for persistence
   - `TryFromEvents`: Reconstruct entities from events
   - `IntoEvents`: Convert new entities into initial events

2. **Proc Macros** (`es-entity-macros/`):
   - `#[derive(EsEvent)]`: Auto-implement event trait
   - `#[derive(EsEntity)]`: Entity boilerplate
   - `#[derive(EsRepo)]`: Generate repository methods including:
     - `create()`: Create new entity
     - `find_by_id()`: Load by ID
     - `list_by_*()`: Query by indexed columns
     - `update()`: Persist entity changes
     - `persist_events()`: Save events to database

3. **Event Storage**:
   - Events stored in PostgreSQL with JSONB
   - Each entity has a dedicated events table
   - Optimistic concurrency control via event sequences
   - Support for idempotent operations

4. **Entity Pattern**:
   - Entities are immutable snapshots
   - State changes produce new events
   - Events are the source of truth
   - Entities can be rebuilt from event history

## Environment Variables

- `SQLX_OFFLINE=true` - Use offline mode for SQLx (required for CI)
- `DATABASE_URL` - PostgreSQL connection string