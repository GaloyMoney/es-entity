//! Runtime (not compile-time-checked) query emission.
//!
//! The Postgres crate emits `sqlx::query!` / `sqlx::query_as!`, which need a
//! live database (or an offline cache) when the *consumer* compiles. The
//! SQLite backend emits runtime queries instead, so a SQLite consumer needs no
//! `DATABASE_URL` at build time. Arguments are encoded eagerly from `&(expr)`
//! exactly as `query!` does, so `expr as Type` casts and borrow semantics are
//! unchanged at the call sites.

use proc_macro2::TokenStream;
use quote::quote;

use crate::sqlite_sql::translate;

/// An expression of type `Result<es_entity::db::Arguments, BoxDynError>`
/// encoding `args` in order.
pub fn args<T: quote::ToTokens>(args: &[T]) -> TokenStream {
    quote! {
        {
            let __args = ::core::result::Result::<_, sqlx::error::BoxDynError>::Ok(
                <es_entity::db::Arguments as ::core::default::Default>::default(),
            );
            #(
                let __args = __args.and_then(|mut __a| {
                    sqlx::Arguments::add(&mut __a, &(#args)).map(move |()| __a)
                });
            )*
            __args
        }
    }
}

/// Like [`args`], but first binds every element of the slice expression
/// `list` (the expanded `= ANY($1)` parameter, see
/// `es_entity::sqlite_write::expand_list_param`).
pub fn args_with_list<T: quote::ToTokens>(list: &TokenStream, rest: &[T]) -> TokenStream {
    quote! {
        {
            let __args = ::core::result::Result::<_, sqlx::error::BoxDynError>::Ok(
                <es_entity::db::Arguments as ::core::default::Default>::default(),
            );
            let __args = (#list).iter().fold(__args, |__acc, __v| {
                __acc.and_then(|mut __a| sqlx::Arguments::add(&mut __a, __v).map(move |()| __a))
            });
            #(
                let __args = __args.and_then(|mut __a| {
                    sqlx::Arguments::add(&mut __a, &(#rest)).map(move |()| __a)
                });
            )*
            __args
        }
    }
}

/// `sqlx::Query` for the (Postgres-dialect) `sql`, translated to SQLite, with
/// `args` bound.
pub fn query<T: quote::ToTokens>(sql: &str, args: &[T]) -> TokenStream {
    let sql = translate(sql);
    let args = self::args(args);
    quote! {
        sqlx::__query_with_result::<es_entity::db::Db, _>(#sql, #args)
    }
}
