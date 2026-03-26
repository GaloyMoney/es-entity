# [cala release v0.10.32](https://github.com/GaloyMoney/cala/releases/tag/0.10.32)



### Refactor

- Use singular entity names for SortBy and Filters types (#115)

# [cala release v0.10.31](https://github.com/GaloyMoney/cala/releases/tag/0.10.31)



### Features

- Add sleep_coalesce for collapsing housekeeping wake-ups (#114)

# [cala release v0.10.30](https://github.com/GaloyMoney/cala/releases/tag/0.10.30)



### Features

- Make entity IDs their own GraphQL scalars (#113)

# [cala release v0.10.29](https://github.com/GaloyMoney/cala/releases/tag/0.10.29)



### Refactor

- Remove set_time, reset_to, and clear_pending_wakes (#112)
- Address PR #108 review feedback (#111)

# [cala release v0.10.28](https://github.com/GaloyMoney/cala/releases/tag/0.10.28)



### Refactor

- Remove auto_advance mode (#108)
- Centralize database types and fix doc typos (#106)

# [cala release v0.10.27](https://github.com/GaloyMoney/cala/releases/tag/0.10.27)



### Bug Fixes

- Return Vec<String> from new_event_types() for sqlx query! compatibility

### Features

- Add event_type() method to EsEvent trait

# [cala release v0.10.26](https://github.com/GaloyMoney/cala/releases/tag/0.10.26)



### Features

- Add post_hydrate_hook support to EsRepo derive macro (#102)

### Miscellaneous Tasks

- Bump pin-project from 1.1.10 to 1.1.11 (#95)
- Bump quote from 1.0.44 to 1.0.45 (#97)
- Bump tokio from 1.49.0 to 1.50.0 (#98)
- Bump uuid from 1.21.0 to 1.22.0 (#101)

### Refactor

- Named parameters for idempotency_guard! macro (#103)

# [cala release v0.10.25](https://github.com/GaloyMoney/cala/releases/tag/0.10.25)



### Bug Fixes

- Rename internal generic 'E' to '__EsErr' to avoid conflicts (#100)

# [cala release v0.10.24](https://github.com/GaloyMoney/cala/releases/tag/0.10.24)



### Bug Fixes

- Resolve generic nested repo error types without generics… (#99)

# [cala release v0.10.23](https://github.com/GaloyMoney/cala/releases/tag/0.10.23)



### Bug Fixes

- Fmt

### Features

- Prefer Display over Debug for NotFound error values

# [cala release v0.10.22](https://github.com/GaloyMoney/cala/releases/tag/0.10.22)



### Features

- Column enum in FindError::NotFound for granular pattern matching (#96)

# [cala release v0.10.21](https://github.com/GaloyMoney/cala/releases/tag/0.10.21)



### Refactor

- [**breaking**] Replace monolithic EsRepoError with per-repo per-operation … (#93)

# [cala release v0.10.20](https://github.com/GaloyMoney/cala/releases/tag/0.10.20)



### Bug Fixes

- Add Cargo.lock to work with crane

### Features

- Soft without queries (#92)

# [cala release v0.10.19](https://github.com/GaloyMoney/cala/releases/tag/0.10.19)



### Features

- Selective list_for(by(...)) syntax (#87)

# [cala release v0.10.18](https://github.com/GaloyMoney/cala/releases/tag/0.10.18)



### Miscellaneous Tasks

- Upgrade async-graphql to 8.0.0-rc.3 (#85)

# [cala release v0.10.17](https://github.com/GaloyMoney/cala/releases/tag/0.10.17)



### Bug Fixes

- Retry database connection in setup-db
- Handle custom accessors correctly in batch operations

# [cala release v0.10.16](https://github.com/GaloyMoney/cala/releases/tag/0.10.16)



### Features

- Update_all (#84)

# [cala release v0.10.15](https://github.com/GaloyMoney/cala/releases/tag/0.10.15)



### Features

- Multi filter (#83)

### Miscellaneous Tasks

- Update convert_case requirement from 0.10 to 0.11 (#81)

# [cala release v0.10.14](https://github.com/GaloyMoney/cala/releases/tag/0.10.14)



### Bug Fixes

- Use crate attribute for graphql feature

### Miscellaneous Tasks

- Bump async-graphql

# [cala release v0.10.13](https://github.com/GaloyMoney/cala/releases/tag/0.10.13)


### Miscellaneous Tasks

- Clock for non op fns (#80)

# [cala release v0.10.12](https://github.com/GaloyMoney/cala/releases/tag/0.10.12)


### Miscellaneous Tasks

- Bump deps (#79)

# [cala release v0.10.11](https://github.com/GaloyMoney/cala/releases/tag/0.10.11)


### Miscellaneous Tasks

- Serde support for artificial clock config (#78)
- Add today fn

# [cala release v0.10.10](https://github.com/GaloyMoney/cala/releases/tag/0.10.10)


### Miscellaneous Tasks

- Use artificial_now to reduce logic (#77)

# [cala release v0.10.9](https://github.com/GaloyMoney/cala/releases/tag/0.10.9)


### Bug Fixes

- Spelling

### Features

- More advanced clock implementation (#76)

### Miscellaneous Tasks

- Bump flake (#75)

# [cala release v0.10.8](https://github.com/GaloyMoney/cala/releases/tag/0.10.8)


### Bug Fixes

- Remove unsupported Into for operations

# [cala release v0.10.7](https://github.com/GaloyMoney/cala/releases/tag/0.10.7)


### Bug Fixes

- Add use stmt

### Miscellaneous Tasks

- Always expose TracingContext struct

# [cala release v0.10.6](https://github.com/GaloyMoney/cala/releases/tag/0.10.6)


### Documentation

- More extensive docs on hooks (#74)

### Miscellaneous Tasks

- Add error field in traces for better navigation (#70)
- Add pre / post commit hooks to DbOp (#71)
- Update convert_case requirement from 0.9 to 0.10 (#65)
- Update darling requirement from 0.21 to 0.23 (#69)
- Remove unused file (#66)
- Remove multilingual field (#67)
- Update convert_case requirement from 0.8 to 0.9 (#63)

### Refactor

- [**breaking**] Rename Ignored -> AlreadyApplied (#73)
- [**breaking**] Update handling of time in AtomicOperation trait (#72)

# [cala release v0.10.5](https://github.com/GaloyMoney/cala/releases/tag/0.10.5)


### Refactor

- Use explicit id name in tracing (#64)

# [cala release v0.10.4](https://github.com/GaloyMoney/cala/releases/tag/0.10.4)


### Documentation

- Fix idempotency chapter indentation

### Miscellaneous Tasks

- Trace fine tuning (#62)

# [cala release v0.10.3](https://github.com/GaloyMoney/cala/releases/tag/0.10.3)


### Miscellaneous Tasks

- Fix instrumentation for retry (#61)

# [cala release v0.10.2](https://github.com/GaloyMoney/cala/releases/tag/0.10.2)



# [cala release v0.10.1](https://github.com/GaloyMoney/cala/releases/tag/0.10.1)


### Miscellaneous Tasks

- More iteration on otel (#59)

# [cala release v0.10.0](https://github.com/GaloyMoney/cala/releases/tag/0.10.0)


### Features

- Adding instrumentation (#57)

# [cala release v0.9.5](https://github.com/GaloyMoney/cala/releases/tag/0.9.5)


### Miscellaneous Tasks

- Upgrade deps (#58)

# [cala release v0.9.4](https://github.com/GaloyMoney/cala/releases/tag/0.9.4)


### Miscellaneous Tasks

- Expose TracingContext (#55)

# [cala release v0.9.3](https://github.com/GaloyMoney/cala/releases/tag/0.9.3)


### Miscellaneous Tasks

- Making simulation config mandatory (#52)

# [cala release v0.9.2](https://github.com/GaloyMoney/cala/releases/tag/0.9.2)


### Miscellaneous Tasks

- Add len_new

# [cala release v0.9.1](https://github.com/GaloyMoney/cala/releases/tag/0.9.1)


### Documentation

- Bump version in book / README

### Miscellaneous Tasks

- Add `find_map` functions to Nested (#49)

# [cala release v0.9.0](https://github.com/GaloyMoney/cala/releases/tag/0.9.0)


### Miscellaneous Tasks

- Bump deps in book
- Declare crate for serde
- Declare crate for schemars feature

# [cala release v0.8.1](https://github.com/GaloyMoney/cala/releases/tag/0.8.1)


### Miscellaneous Tasks

- Widen version requirements for sqlx

# [cala release v0.8.0](https://github.com/GaloyMoney/cala/releases/tag/0.8.0)


### Refactor

- Rename tracing feature to tracing-context

# [cala release v0.7.19](https://github.com/GaloyMoney/cala/releases/tag/0.7.19)


### Miscellaneous Tasks

- Update darling requirement from 0.20 to 0.21 (#46)
- Add iter_persisted in nested (#47)

# [cala release v0.7.18](https://github.com/GaloyMoney/cala/releases/tag/0.7.18)


### Documentation

- Fix readme events table

### Miscellaneous Tasks

- Remove Cargo.lock (#37)

### Performance

- Add persist_event_context to disable sending redundant data

# [cala release v0.7.17](https://github.com/GaloyMoney/cala/releases/tag/0.7.17)


### Bug Fixes

- Strip alias in es_query order by columns

### Miscellaneous Tasks

- Fmt

# [cala release v0.7.16](https://github.com/GaloyMoney/cala/releases/tag/0.7.16)


### Features

- Load context (#36)

# [cala release v0.7.15](https://github.com/GaloyMoney/cala/releases/tag/0.7.15)


### Miscellaneous Tasks

- Add tracing to event-context (#35)

# [cala release v0.7.14](https://github.com/GaloyMoney/cala/releases/tag/0.7.14)


### Miscellaneous Tasks

- Add tracing data to event context (#34)

# [cala release v0.7.13](https://github.com/GaloyMoney/cala/releases/tag/0.7.13)


### Documentation

- Small re-wording for entity_id

### Features

- Event context (#33)

### Miscellaneous Tasks

- Bump flake
- Add sim-time
- Document sim-time
- One_time_executor details

### Refactor

- SimTimeConfig -> SimulationConfig

# [cala release v0.7.12](https://github.com/GaloyMoney/cala/releases/tag/0.7.12)


### Miscellaneous Tasks

- One_time_executor and operation

# [cala release v0.7.11](https://github.com/GaloyMoney/cala/releases/tag/0.7.11)


### Bug Fixes

- Make #[es_repo(nested)] work under async_trait

# [cala release v0.7.10](https://github.com/GaloyMoney/cala/releases/tag/0.7.10)


### Bug Fixes

- Use <#ty> to handle generic children in nested.rs

# [cala release v0.7.9](https://github.com/GaloyMoney/cala/releases/tag/0.7.9)


### Miscellaneous Tasks

- Use AsRef<str> for String columns
- Support AsRef<str> in find_by

# [cala release v0.7.8](https://github.com/GaloyMoney/cala/releases/tag/0.7.8)


### Bug Fixes

- Declare op as <'static>

# [cala release v0.7.7](https://github.com/GaloyMoney/cala/releases/tag/0.7.7)


### Miscellaneous Tasks

- Add tx_mut fn to DbOp (#30)

# [cala release v0.7.6](https://github.com/GaloyMoney/cala/releases/tag/0.7.6)


### Miscellaneous Tasks

- Bump flake

# [cala release v0.7.5](https://github.com/GaloyMoney/cala/releases/tag/0.7.5)


### Miscellaneous Tasks

- Add internals to repo-list-for-filter
- Update doc links and fix license file name (#28)

### Refactor

- Find_many -> list_for_filter (#29)

# [cala release v0.7.4](https://github.com/GaloyMoney/cala/releases/tag/0.7.4)


### Miscellaneous Tasks

- #![forbid(unsafe_code)]
- Update README

# [cala release v0.7.3](https://github.com/GaloyMoney/cala/releases/tag/0.7.3)


### Bug Fixes

- README private child struct

### Miscellaneous Tasks

- Reset mdbook-test profile in ci
- Remove holucination from readme
- Add event module docs (#25)
- Readme

### Performance

- Use uuid v7 (#27)

# [cala release v0.7.2](https://github.com/GaloyMoney/cala/releases/tag/0.7.2)


### Miscellaneous Tasks

- Bump flake
