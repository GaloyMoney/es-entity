# Introduction

Welcome to the ES Entity documentation!

ES Entity is an opinionated rust library for persisting Event Sourced entities to PostgreSQL.

It promotes decoupling your domain code from persistence details by putting all the mapping logic onto `Repository` structs.
Almost all the generated queries are verified at compile time by [`sqlx`](https://crates.io/crates/sqlx) under the hood to give strong type-safe guarantees.

The main traits that must to be derived are `EsEvent` and `EsEntity` so that they can be used by the `EsRepo` macro that generates all the persistence and query fns.

This book will explain how to use this library effectively as well as provide a general introduction on how to use Event Sourcing to persist the state of your domain entities.
