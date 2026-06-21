# Unsafe Code Inventory — zerotrace-kernel

> Policy: Firecracker standard.  Every unsafe block must have a `// SAFETY:`
> comment, a proof sketch, and a link to the test(s) that cover it.  This file
> is the canonical index.  See `src/lib.rs` for the crate-level deny rule.

| # | Location | Operation | Invariant | Test(s) | MIRI |
|---|---|---|---|---|---|
| — | — | — | — | — | — |

## Status: No unsafe code

As of the `Arc::downcast` stabilisation (Rust 1.76), the two unsafe blocks
referenced in the original design have been replaced with safe equivalents:

- `World::get` / `World::_get`: uses `Arc::downcast::<RwLock<T>>()` (safe)
- `World::get_raw` / `World::_get_raw`: uses `Arc::downcast::<T>()` (safe)
- `World::remove_resource` / `World::remove_keyed`: uses `Arc::downcast` (safe)

The `TypeId`-keyed insertion and retrieval still provide the same type-safety
guarantee: insertion by `TypeId::of::<T>()` and retrieval by the same `TypeId`
ensures the erased `Arc<dyn Any>` contains a value of type `T`.  The safe
`Arc::downcast` API performs the same pointer-equality check internally that
the old `Arc::into_raw` / `Arc::from_raw` pattern did manually.

## Proof sketch for World::get / World::get_raw

1. `World::insert::<T>(value)` stores `Arc::new(RwLock::new(value))` keyed by
   `TypeId::of::<T>()`.
2. `World::get::<T>()` reads the entry for `TypeId::of::<T>()` and retrieves
   the `Arc<dyn Any + Send + Sync>`.
3. Because (a) the `TypeId` uniquely identifies `T` (compiler guarantee), and
   (b) the insertion and retrieval use the same `T`, the erased `Arc` is
   guaranteed to contain a value of type `RwLock<T>`.
4. `Arc::downcast::<RwLock<T>>()` performs an internal `TypeId` comparison and
   returns the correctly-typed `Arc<RwLock<T>>` on match.

## MIRI status

Not applicable — the crate contains no unsafe blocks.

## Historical note

The original design (before Rust 1.76) used the following unsafe pattern:

```rust
let raw = Arc::into_raw(any) as *const T;
let arc = unsafe { Arc::from_raw(raw) };
```

This was sound because `Arc::into_raw` / `Arc::from_raw` on the same pointer
preserves the reference count, and the `TypeId` check ensures the pointee is
of type `T`.  The `Arc::downcast` stabilisation (tracking issue
[#71855](https://github.com/rust-lang/rust/issues/71855)) made this pattern
obsolete.
