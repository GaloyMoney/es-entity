//! Small type-inspection helpers shared across derives.

/// Check if a type's last path segment is "Forgettable".
///
/// Shared by the `EsEvent` and `EsSnapshot` derives: both detect top-level
/// `Forgettable<T>` fields the same way. Does not see `Forgettable` nested
/// inside another type (e.g. `Vec<Forgettable<T>>`) — that limitation is
/// documented, not special-cased.
pub(crate) fn is_forgettable_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident == "Forgettable";
    }
    false
}
