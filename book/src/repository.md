# Repository

In the context of `es-entity` a `Repository` is a `struct` that hosts all operations performed with the database.
Thus it is responsible for all CRUD style interactions with the persistence layer.

The `EsRepo` macro generates functions such as:
- `create`
- `update`
- `find_by_id`
- `list_by_id`
- etc.

that hide away the complexity of querying and hydrating entities who's state is represented in an Event Sourced way.

Under the hood the `es_query!` helper macro (which only works within `fn`s inside `EsRepo` structs) handles loading the events while enabling you to write a 'normal' looking SQL query against the `index` table.

The following sections will take a deep dive in how this design came to be.
