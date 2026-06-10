# Unsafe Code Inventory — zerotrace-kernel

> Policy: Firecracker standard.  Every unsafe block must have a `// SAFETY:`
> comment, a proof sketch, and a link to the test(s) that cover it.  This file
> is the canonical index.  See `src/lib.rs` for the crate-level deny rule.

| # | Location | Operation | Invariant | Test(s) | MIRI |
|---|---|---|---|---|---|
| 1 | `world.rs` — `World::get` | `Arc<dyn Any>` → `Arc<T>` via `Arc::into_raw` / `Arc::from_raw` | `TypeId::of::<T>()` matches the key used at insertion time; TypeId uniquely identifies a Rust type | `world::tests::test_insert_and_get`, `test_concurrent_reads` | TBD |
| 2 | `world.rs` — `World::remove` | Same as above | Same as above | `world::tests::test_remove` | TBD |

## Proof sketch for `World::get` / `World::remove`

1. `World::insert::<T>(value)` stores `Arc::new(value)` keyed by `TypeId::of::<T>()`.
2. `World::get::<T>()` reads the entry for `TypeId::of::<T>()` and retrieves the `Arc<dyn Any>`.
3. Because (a) the `TypeId` uniquely identifies `T` (compiler guarantee), and
   (b) the insertion and retrieval use the same `T`, the erased `Arc` is
   guaranteed to contain a value of type `T`.
4. `Arc::into_raw` / `Arc::from_raw` on the same pointer preserves the
   reference count — the pointer arithmetic is a no-op, only the static type
   changes.
5. Therefore the downcast is sound.

## MIRI status

- Date last run: TBD (will be run as part of T1.3 acceptance)
- Command: `MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test -p zerotrace-kernel -- world`
- Result: TBD

## When `Arc::downcast` stabilizes

Replace both unsafe blocks with:

```rust
let any = map.get(&TypeId::of::<T>())?.clone();
any.downcast::<T>().ok()
```

Tracking issue: <https://github.com/rust-lang/rust/issues/71855>
