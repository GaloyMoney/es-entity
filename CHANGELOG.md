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
