# Getting Started

This chapter will help you get started with ES Entity.

## Installation

Add ES Entity to your `Cargo.toml`:

```toml
[dependencies]
es-entity = "0.1"
es-entity-macros = "0.1"
```

## Database Setup

ES Entity uses PostgreSQL for event storage. Make sure you have PostgreSQL running and set up your database:

```bash
# Start PostgreSQL with Docker
make start-deps

# Run migrations
make setup-db
```

## Your First Entity

Let's create a simple entity to get started:

```rust
use es_entity::*;

#[derive(EsEvent)]
enum BookEvent {
    Created { title: String, author: String },
    Published,
}

#[derive(EsEntity)]
struct Book {
    id: BookId,
    title: String,
    author: String,
    published: bool,
}
```

Next, we'll explore how to use these entities in your application.