# Architecture

This chapter explains the architecture and design principles of ES Entity.

## Core Concepts

### Event Sourcing

Event sourcing is a pattern where we store all changes to application state as a sequence of events. Instead of storing just the current state, we store the history of how we arrived at that state.

### Events

Events represent facts that have happened in the system. They are immutable and form the source of truth for your application state.

### Entities

Entities are built by replaying events. They represent the current state of your domain objects.

### Repositories

Repositories handle the persistence layer, storing events and reconstructing entities from event streams.

## Design Principles

1. **Immutability**: Events are immutable once created
2. **Event-First**: All state changes must go through events
3. **Type Safety**: Leverage Rust's type system for correctness
4. **Performance**: Efficient event storage and replay

## Database Schema

Events are stored in PostgreSQL with the following structure:

```sql
CREATE TABLE entity_events (
    id UUID PRIMARY KEY,
    entity_id UUID NOT NULL,
    sequence BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    UNIQUE(entity_id, sequence)
);
```

This schema provides:
- Optimistic concurrency control via sequences
- Efficient querying by entity ID
- JSONB storage for flexible event payloads