
//! AST manager
//!
//! The AST manager stores AST nodes, referred to via `ID`. These nodes can
//! be used to represent sorts, terms, formulas, theory terms, etc.

use std::slice;
use super::symbol::Symbol;
use fxhash::FxHashMap;

/* Note: positive IDs are applications, negative IDs are symbols
 */
/// The unique identifier of an AST node.
#[derive(Copy,Clone,Eq,PartialEq,Hash,Ord,PartialOrd,Debug)]
pub struct AST(i32);

/// The definition of an AST node, as seen from outside
#[derive(Debug,Copy,Clone)]
pub enum View<'a> {
    Const(&'a Symbol),
    App {
        f: AST,
        args: &'a [AST],
    }
}

// Definition of application keys
//
// These keys are optimized so that:
// - they don't need any allocation for "small" applications
// - they only need to allocate one Box for "big" applications, shared between
//   the map and vector
mod app_key {
    use super::*;
    use std::marker::PhantomData;

    // Number of arguments for a "small" term application
    const N_SMALL_APP : usize = 3;

    #[derive(Copy,Clone)]
    union ArrOrVec<T : Copy> {
        arr: [T; N_SMALL_APP],
        ptr: * const T, // will be shared between vec and hashmap
    }

    // Main type
    pub(crate) struct T<'a> {
        f: AST,
        len: u16,
        args: ArrOrVec<AST>,
        phantom: PhantomData<&'a ()>,
    }

    fn check_len(len: usize) {
        use std::u16;
        if len > u16::MAX as usize {
            panic!("cannot make an AST application of length {}", len);
        }
    }

    impl T<'static> {
        pub fn f(&self) -> AST { self.f }

        pub fn new(f: AST, args: &[AST]) -> Self {
            let len = args.len();
            check_len(len);

            // copy arguments into local array or heap
            let new_args =
                if len <= N_SMALL_APP {
                    let mut arr = [AST(0); N_SMALL_APP];
                    arr[0..len].copy_from_slice(args);
                    ArrOrVec{arr}
                } else {
                    use std::mem;
                    // go through a vector to allocate on the heap
                    let mut v = Vec::with_capacity(len);
                    v.extend_from_slice(args);
                    let ptr = v.as_slice().as_ptr(); // access the pointer
                    mem::forget(v);
                    ArrOrVec{ptr}
                };
            let r = T {
                f, len: len as u16, args: new_args,
                phantom: PhantomData::default(),
            };
            debug_assert_eq!(r.args(), args, "expected {:?} got {:?}", args, r.args());
            r
        }
    }

    impl<'a> T<'a> {
        #[inline(always)]
        pub fn args<'b: 'a>(&'b self) -> &'b [AST] {
            let len = self.len as usize;
            if len <= N_SMALL_APP {
                unsafe {& self.args.arr[..len]}
            } else {
                unsafe {slice::from_raw_parts(self.args.ptr, self.len as usize)}
            }
        }

        // Temporary-lived key, borrowing the given slice
        pub fn mk_ref(f: AST, args: &'a [AST]) -> Self {
            let len = args.len();
            check_len(len);
            let new_args =
                if len <= N_SMALL_APP {
                    let mut arr = [AST(0); N_SMALL_APP];
                    arr[0..len].copy_from_slice(args);
                    ArrOrVec{arr}
                } else {
                    ArrOrVec{ptr: args.as_ptr()}
                };
            let r = T {
                f, len: len as u16, args: new_args,
                phantom: PhantomData::default(),
            };
            debug_assert_eq!(r.args(), args, "expected {:?} got {:?}", args, r.args());
            r
        }

        pub fn to_owned(self) -> T<'static> {
            T::new(self.f, self.args())
        }
    }

    impl Clone for T<'static> {
        fn clone(&self) -> Self {
            let &T{f, len, args, phantom} = self;
            T{f,len,args,phantom}
        }
    }

    impl<'a> Eq for T<'a> {}
    impl<'a> PartialEq for T<'a> {
        fn eq(&self, other: &T<'a>) -> bool {
            self.f == other.f && self.args() == other.args()
        }
    }

    use std::hash::{Hash,Hasher};

    impl<'a> Hash for T<'a> {
        fn hash<H:Hasher>(&self, h: &mut H) {
            self.f.hash(h);
            self.args().hash(h)
        }
    }

    use std::fmt::{Debug,self};

    impl<'a> Debug for T<'a> {
        fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
            write!(fmt, "({:?} {:?})", self.f, self.args())
        }
    }
}

pub struct AstManager {
    consts: Vec<Symbol>,
    apps: Vec<app_key::T<'static>>,
    tbl_app: FxHashMap<app_key::T<'static>, AST>, // for hashconsing
}

impl AstManager {
    /// Create a new AST manager
    pub fn new() -> Self {
        AstManager {
            consts: Vec::with_capacity(512),
            apps: Vec::with_capacity(1_024),
            tbl_app: FxHashMap::default(),
        }
    }

    /// View the definition of an AST node
    #[inline]
    pub fn view(&self, ast: AST) -> View {
        if ast.0 < 0 {
            let s = & self.consts[((- ast.0)-1) as usize];
            View::Const(s)
        } else {
            let k = & self.apps[ast.0 as usize];
            View::App {f: k.f(), args: k.args()}
        }
    }

    fn mk_symbol(&mut self, s: Symbol) -> AST {
        let n = - (1 + self.consts.len() as i32);
        self.consts.push(s);
        AST(n)
    }

    /// Make a named symbol.
    ///
    /// Note that calling this function twice with the same string
    /// will result in two distinct symbols (as if the second one
    /// was shadowing the first). Use an auxiliary hashtable if
    /// you want sharing.
    #[inline]
    pub fn mk_const(&mut self, s: &str) -> AST {
        self.mk_symbol(Symbol::mk_str(s.to_string()))
    }

    pub fn mk_app(&mut self, f: AST, args: &[AST]) -> AST {
        let k = app_key::T::mk_ref(f, args);

        // borrow multiple fields
        let apps = &mut self.apps;
        let tbl_app = &mut self.tbl_app;
        //let AstManager {ref mut apps, ref mut tbl_app,..} = self;

        let ast =
            match tbl_app.get(&k) {
                Some(a) => Ok(*a), // fast path
                None => Err(()),
            };

        match ast {
            Ok(a) => a,
            Err(_) => {
                // insert
                let n = apps.len();
                let ast = AST(n as i32);
                // make 2 owned copies of the key
                let k1 = k.to_owned();
                let k2 = k1.clone();
                apps.push(k1);
                tbl_app.insert(k2, ast);
                // return AST
                ast
            }
        }
    }
}
