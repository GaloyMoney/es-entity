# Introduction

Welcome to the ES Entity Framework documentation!

ES Entity is an Event Sourcing Entity Framework for Rust that provides a powerful and flexible way to build event-sourced applications.

## Features

- **Event Sourcing**: Store all changes to application state as a sequence of events
- **Entity Pattern**: Build entities from event streams
- **Type Safety**: Leverage Rust's type system for compile-time guarantees
- **PostgreSQL Storage**: Reliable persistence with JSONB support
- **Proc Macros**: Reduce boilerplate with derive macros

## Quick Example

```rust
use es_entity::*;

#[derive(EsEvent)]
enum UserEvent {
    Created { name: String },
    NameChanged { new_name: String },
}

#[derive(EsEntity)]
struct User {
    id: UserId,
    name: String,
}
```

This book will guide you through everything you need to know to use ES Entity effectively.