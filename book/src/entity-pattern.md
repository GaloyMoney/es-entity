# Entity Pattern

In the Software Engineering community the term `Entity` can refer to many different things.
In the context of `es-entity` it is generally meant in the sense put forward by Domain Driven Design.
Strict adherence to DDD is not mandatory to use `es-entity` but there are a lot of benefits to be had by following these principles.

In DDD entities serve the following purpose:
- execute commands that
  - execute business logic
  - enforce domain invariants
  - mutate state
  - record events (in the context of Event Sourcing)
- supply queries that expose some of the entities state

They often host the most critical code in your application where correctness is of upmost importance.
Ideally they are unit-testable and thus should not be overly coupled to the persistence layer (as they generally are when using just about any ORM library / framework).
The design of `es-entity` is very deliberate in not getting in the way of testability of your `Entities`.

Each `Entity` type used in `es-entity` must have:
- an `EntityId` type
- an `EntityEvent` type
- a `NewEntity` type
- the `Entity` type itself
