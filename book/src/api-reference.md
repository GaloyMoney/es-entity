# API Reference

This chapter provides a detailed reference for the ES Entity API.

## Traits

### `EsEvent`

The core trait for events:

```rust
pub trait EsEvent: Serialize + DeserializeOwned {
    fn event_type(&self) -> &'static str;
}
```

### `EsEntity`

The trait for entities:

```rust
pub trait EsEntity: Sized {
    type Event: EsEvent;
    
    fn id(&self) -> EntityId;
    fn events(&self) -> &[Self::Event];
}
```

### `EsRepo`

Repository trait for persistence:

```rust
pub trait EsRepo {
    type Entity: EsEntity;
    
    async fn create(&self, entity: Self::Entity) -> Result<()>;
    async fn find_by_id(&self, id: EntityId) -> Result<Self::Entity>;
    async fn update(&self, entity: Self::Entity) -> Result<()>;
}
```

## Derive Macros

### `#[derive(EsEvent)]`

Automatically implements the `EsEvent` trait:

```rust
#[derive(EsEvent)]
enum MyEvent {
    Created { name: String },
    Updated { name: String },
}
```

### `#[derive(EsEntity)]`

Generates entity boilerplate:

```rust
#[derive(EsEntity)]
struct MyEntity {
    id: EntityId,
    name: String,
}
```

### `#[derive(EsRepo)]`

Creates repository implementation:

```rust
#[derive(EsRepo)]
struct MyRepo {
    pool: PgPool,
}
```