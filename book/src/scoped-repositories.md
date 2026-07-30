# Scoped Repositories

Multi-tenant applications need data access that is impossible to misuse: a
query that *forgets* to filter by the tenant column is a silent cross-tenant
leak that compiles and passes all same-tenant tests. Scoped repositories make
that mistake unrepresentable — on a scoped repo, **every generated read
function requires a scope argument**, enforced by the compiler.

## Declaring a scope column

Mark exactly one column with `scope`:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "Customer",
    columns(
        partner_id(ty = "PartnerId", scope),
        email(ty = "String"),
    )
)]
pub struct Customers {
    pool: PgPool,
}
```

`partner_id` remains an ordinary persisted column — it is populated on
`create` from the `NewCustomer`'s field like any other column. The `scope`
marker additionally generates an entity-named scope enum:

```rust,ignore
pub enum CustomerScope {
    All,               // no filter — reads across all scopes
    Only(PartnerId),   // restricts every read to this scope value
}

impl From<PartnerId> for CustomerScope { /* => Only */ }
impl From<&PartnerId> for CustomerScope { /* => Only */ }
```

There is deliberately **no** `From<Option<PartnerId>>`: mapping `None` to
`All` would turn a stray `None` into silent all-scope access. All-scope reads
must be written explicitly — `CustomerScope::All` is greppable and auditable.

## The scoped read surface

Every generated read function gains a leading `scope: impl Into<{Entity}Scope>`
argument:

```rust,ignore
customers.find_by_id(partner_id, id).await?;              // Into => Only
customers.find_by_id(CustomerScope::All, id).await?;      // explicit escape hatch
customers.maybe_find_by_email(partner_id, email).await?;
customers.find_all::<Customer>(partner_id, &ids).await?;
customers.list_by_created_at(partner_id, args, direction).await?;
customers.list_for_filters(partner_id, filters, sort, args).await?;

customers.find_by_id(id).await?;  // does not exist — compile error
```

At runtime each function dispatches between two static, compile-time-checked
SQL variants:

- `All` executes exactly the SQL an unscoped repo would.
- `Only(value)` executes a variant with an additional `partner_id = $n`
  conjunct in every `WHERE` clause.

Both arms are plain equality predicates — sargable against a scope-column-led
index (see below). Under `Only`, a row from another scope behaves exactly like
a missing row: `find_by_*` returns `NotFound`, `maybe_find_by_*` returns
`None`, `find_all` silently omits the id, and lists never contain the row.
**Missing and not-yours look identical.**

## The bound view: `repo.scoped(scope)`

When a request performs several reads under one subject, threading the scope
into every call gets repetitive. Scoped repos additionally generate a
**bound view** — `Scoped{Repo}` — that captures the scope once:

```rust,ignore
let customers = self.customers.scoped(sub.scope());   // ScopedCustomers<'_>

customers.find_by_id(id).await?;                      // no per-call scope arg
customers.maybe_find_by_email(email).await?;
customers.list_by_created_at(args, direction).await?;
customers.find_by_id_in_op(&mut op, id).await?;       // _in_op variants too
customers.scope();                                    // the bound CustomerScope
```

Every view method simply delegates to the corresponding scope-argument fn
with the bound scope — no new SQL, identical semantics. The view **borrows**
the repository (`ScopedCustomers<'a>` holds `&'a Customers`), so it is
naturally request-scoped: it cannot be stored beyond the repo borrow, which
keeps a bound all-access or tenant view from quietly outliving the request
that justified it.

## Writes are custody-guarded

`create`, `create_all`, `update`, `update_all` and `delete` keep their
unscoped signatures. The reasoning: mutations operate on an entity value that
could only have been obtained through a scoped read (or built by domain logic
that stamped the scope column). Scope enforcement happens at the boundary that
turns ids and queries into entity data; once you hold the entity, custody of
the value is the guarantee.

## Cursors carry no filter authority

Pagination cursors are position markers only. Every page executes with the
scope conjunct in its own `WHERE` clause, so a tampered, fabricated, or
foreign cursor can only reposition pagination within the caller's own scoped
rows — it can never widen the result set, and cursor values are compared, not
dereferenced, so they cannot be used to probe for the existence of foreign
ids. Replaying a cursor minted under a different scope yields well-defined
(scoped) but position-shifted results.

## Validation rules

The macro rejects at compile time:

- more than one `scope` column per repo
- an `Option<T>` or `nullable`-annotated scope column (nullable scope columns
  are not supported — every row must belong to exactly one scope)
- a `Forgettable<T>` scope column
- `find_by`, `list_by` or `list_for` on the scope column itself — every read
  is already filtered by it; per-scope listing *is* the ordinary scoped
  `list_by_*(Only(value), ..)`
- `scope` on nested repos — children are custody-guarded via their (scoped)
  parent

The scope column also generates no `find_by_partner_id` accessors: the scope
argument replaces them.

## Index requirements

The `Only` arm adds a leading equality on the scope column to every read, so
composite indexes should lead with it:

```sql
-- list_by_created_at under Only(p)
CREATE INDEX ON customers (partner_id, created_at DESC, id DESC);

-- list_for_status_by_created_at under Only(p)
CREATE INDEX ON customers (partner_id, status, created_at DESC, id DESC);

-- find_by_email under Only(p)
CREATE INDEX ON customers (partner_id, email);
```

Plain single-column indexes keep working (Postgres can still apply the scope
conjunct as an index qual or filter), but scope-led composites let the
paginated lists ride the index order with an early-exit `LIMIT`.
