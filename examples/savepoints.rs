//! Nested savepoint examples.
//!
//! Transactions nest to arbitrary depth. Each savepoint is independently
//! committable — roll back an inner scope without losing outer work.

use fastarena::Arena;

fn main() {
    // --- Basic nested savepoint: inner dropped, outer survives ---
    let mut arena = Arena::new();
    {
        let mut outer = arena.transaction();
        let _parser_ast = outer.alloc_str("top-level");

        {
            let mut inner = outer.savepoint();
            inner.alloc_str("speculative-opt");
            inner.alloc(999u32);
            println!("  inner depth={}", inner.depth());
            // dropped without commit — inner work discarded, outer untouched
        }

        let _final_ast = outer.alloc_str("confirmed");
        outer.commit();
        println!("outer committed successfully");
    }

    // --- Three levels of nesting ---
    arena.reset();
    {
        let mut t1 = arena.transaction();
        println!("depth after t1: {}", t1.arena_depth());
        t1.alloc(1u32);

        {
            let mut t2 = t1.savepoint();
            t2.alloc(2u32);
            println!("t2 depth={}", t2.depth());

            {
                let mut t3 = t2.savepoint();
                t3.alloc(3u32);
                println!("t3 depth={}", t3.depth());
                t3.commit();
            }

            println!("t2 arena_depth after t3 commit: {}", t2.arena_depth());
            t2.commit();
        }

        println!("t1 arena_depth after t2 commit: {}", t1.arena_depth());
        t1.commit();
    }
    println!("Final depth: {}", arena.transaction_depth());

    // --- Partial rollback across blocks ---
    arena.reset();
    {
        let mut outer = arena.transaction();
        outer.alloc(100u64);
        let after_outer = outer.bytes_used();

        {
            let mut sp = outer.savepoint();
            for _ in 0..25 {
                sp.alloc(0u64);
            }
            // savepoint dropped — rolled back
        }

        assert_eq!(outer.bytes_used(), after_outer);
        println!("Savepoint rollback preserved parent: {} bytes", after_outer);
        outer.commit();
    }
}
