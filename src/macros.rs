/// Creates an [`ArenaVec`] backed by the given arena.
///
/// `arenavec!` allows `ArenaVec`s to be defined with a concise syntax.
/// The arena reference always follows the `in` keyword.
///
/// # Forms
///
/// **Empty:**
/// ```rust
/// # use fastarena::{Arena, ArenaVec, arenavec};
/// let mut arena = Arena::new();
/// let v: ArenaVec<u32> = arenavec![in &mut arena];
/// assert!(v.is_empty());
/// ```
///
/// **List of elements:**
/// ```rust
/// # use fastarena::{Arena, arenavec};
/// let mut arena = Arena::new();
/// let v = arenavec![in &mut arena; 1u32, 2, 3];
/// assert_eq!(v.as_slice(), &[1, 2, 3]);
/// ```
///
/// **Repeated element** (requires `T: Clone`):
/// ```rust
/// # use fastarena::{Arena, arenavec};
/// let mut arena = Arena::new();
/// let v = arenavec![in &mut arena; 0u32; 10];
/// assert_eq!(v.len(), 10);
/// assert_eq!(v.as_slice(), &[0; 10]);
/// ```
#[macro_export]
macro_rules! arenavec {
    // Empty
    (in $arena:expr) => {
        $crate::ArenaVec::new($arena)
    };

    // List (with trailing comma)
    (in $arena:expr; $($elem:expr),+ $(,)?) => {{
        let mut v = $crate::ArenaVec::new($arena);
        $( v.push($elem); )*
        v
    }};

    // Repeat
    (in $arena:expr; $elem:expr; $count:expr) => {{
        let mut v = $crate::ArenaVec::with_capacity($arena, $count);
        v.extend_exact(::core::iter::repeat($elem).take($count));
        v
    }};
}
