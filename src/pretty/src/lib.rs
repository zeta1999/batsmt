
//! Pretty printing infrastructure
//!
//! Objects that can be rendered nicely as trees should implement `Pretty`.
//! This way, they get a `Display` instance for free.

extern crate pretty;

use {
    std::{
        fmt, borrow::{Borrow,ToOwned},
        collections::VecDeque,
    },
    pretty::{DocAllocator,DocBuilder,Doc,Arena},
};

// operations
#[derive(Debug,Clone)]
enum Op {
    Open(usize),
    Close,
    Newline,
    Space,
    SStatic(&'static str),
    Text(String),
}

/// The context used to print objects
pub struct Ctx {
    alternate: bool, // alternate (more verbose) mode
    ops: VecDeque<Op>,
}

type StackItem<'a> = DocBuilder<'a, Arena<'a,()>>;

// a stack of document builders
struct Stack<'a> {
    pub st: Vec<StackItem<'a>>, // queue of operations
    pub boxes: Vec<usize>, // indentation levels
}

impl<'a> Stack<'a> {
    fn new() -> Self {
        Stack { st: Vec::new(), boxes: Vec::new(), }
    }

    fn enter_box(&mut self, n: usize, start: StackItem<'a>) {
        self.boxes.push(n);
        self.st.push(start); // to be combined with the rest
    }
    fn exit_box(&mut self) -> usize {
        debug_assert!(self.boxes.len() > 0);
        self.boxes.pop().expect("no box to exit")
    }

    // push `d` onto the stack
    fn push(&mut self, d: StackItem<'a>) {
        // in a box?
        if self.boxes.len() > 0 {
            match self.st.pop() {
                None => self.st.push(d),
                Some(d2) => self.st.push(d2.append(d)),
            }
        } else {
            self.st.push(d);
        }
    }

    fn pop(&mut self) -> StackItem<'a> {
        self.st.pop().expect("cannot pop from empty stack")
    }

    // assuming there's only one element remaining, pop it
    fn pop_last(&mut self) -> StackItem<'a> {
        debug_assert!(self.boxes.len() == 0, "all boxes should be closed");
        if self.st.len() == 1 {
            self.st.pop().unwrap()
        } else {
            panic!("pretty: ill formed document (expected 1 doc, not {})",
                self.st.len())
        }
    }
}

impl Ctx {
    // Allocate a new local printing context
    fn new() -> Self {
        Ctx { alternate: false, ops: VecDeque::new(), }
    }

    /// Is the context in alternate mode?
    ///
    /// Alternate mode should be more verbose, typically used for debug.
    pub fn alternate(&self) -> bool { self.alternate }
    fn set_alternate(&mut self) { self.alternate = true }

    fn into_str(mut self, width: usize) -> String {
        let arena = Arena::new();

        // wrap into toplevel box
        self.ops.push_front(Op::Open(0));
        self.ops.push_back(Op::Close);

        // temporary docs
        let mut stack = Stack::new();

        while let Some(op) = self.ops.pop_front() {
            //println!("process op {:?} (stack len {} nboxes {})", op, stack.st.len(), stack.boxes.len());

            match op {
                Op::Open(n) => {
                    stack.enter_box(n, arena.nil());
                },
                Op::Newline => {
                    stack.push(arena.newline());
                },
                Op::Space => {
                    stack.push(arena.space());
                },
                Op::Close => {
                    let n = stack.exit_box();
                    let mut d = stack.pop();
                    if n > 0 { d = d.nest(n) }
                    d = d.group();
                    stack.push(d) // might combine with previous box
                },
                Op::SStatic(str) => {
                    stack.push(arena.text(str));
                },
                Op::Text(s) => {
                    stack.push(arena.text(s));
                },
            }
        }

        // extract top doc
        let d : Doc<_> = stack.pop_last().into();

        // render to a string
        let mut s = Vec::new();
        d.render(width, &mut s).unwrap();
        String::from_utf8(s).unwrap()
    }
}

// Re-export stuff from the pretty printer lib
impl Ctx {
    fn push_(&mut self, op: Op) -> &mut Self { self.ops.push_back(op); self }
    pub fn str(&mut self, s: &'static str) -> &mut Self { self.push_(Op::SStatic(s)) }
    pub fn text<U>(&mut self, u: &U) -> &mut Self
        where U:ToOwned<Owned=String>, String:Borrow<U>
    { self.push_(Op::Text(u.to_owned())) }
    pub fn string(&mut self, s: String) -> &mut Self { self.push_(Op::Text(s)) }
    pub fn newline(&mut self) -> &mut Self { self.push_(Op::Newline) }
    pub fn space(&mut self) -> &mut Self { self.push_(Op::Space) }
    fn open_indent(&mut self, u: usize) -> &mut Self { self.push_(Op::Open(u)); self }
    fn close(&mut self) -> &mut Self { self.push_(Op::Close); self }

    pub fn pp<T:Pretty>(&mut self, x: &T) -> &mut Self { x.pp_into(self); self }
    pub fn pp1<T:Pretty1<U>,U>(&mut self, x: &T, y: &U) -> &mut Self { x.pp1_into(y, self); self }
    pub fn pp2<T:Pretty2<U,V>,U,V>(&mut self, x: &T, y: &U, z: &V) -> &mut Self { x.pp2_into(y,z,self); self }

    /// Call `f` in a box with given indentation
    pub fn with_indent<F,U>(&mut self, n: usize, f: F) -> &mut Self
        where F: FnOnce(&mut Ctx) -> U
    {
        self.open_indent(n);
        f(self);
        self.close();
        self
    }

    pub fn with_box<F>(&mut self, f: F) -> &mut Self where F: FnOnce(&mut Ctx) { self.with_indent(0,f) }

    pub fn sexp<F,U>(&mut self, f: F) -> &mut Self
        where F: FnOnce(&mut Ctx) -> U
    { self.str("("); self.with_indent(1,f); self.str(")"); self }

    /// Print `t` using its debug implementation.
    pub fn debug<T>(&mut self, x: T) -> &mut Self where T: fmt::Debug {
        self.string(format!("{:?}", x))
    }

    /// Print `t` using its display implementation.
    pub fn display<T>(&mut self, x: T) -> &mut Self where T: fmt::Display {
        self.string(format!("{}", x))
    }

    /// `ctx.array(sep, arr)` prints elements of `arr` with `str` in between
    pub fn array<Sep: Pretty, U:Pretty>(&mut self, sep: Sep, arr: &[U]) -> &mut Self
    {
        for (i,x) in arr.iter().enumerate() {
            if i > 0 { sep.pp_into(self); }
            x.pp_into(self)
        }
        self
    }

    /// `ctx.array(sep, arr)` prints elements of `arr` with `str` in between
    pub fn iter<Sep, I, U>(&mut self, sep: Sep, iter: I) -> &mut Self
        where Sep: Pretty, U: Pretty, I: Iterator<Item=U>
    {
        for (i,x) in iter.enumerate() {
            if i > 0 { sep.pp_into(self); }
            x.pp_into(self)
        }
        self
    }
}

/// Default printing width, in case one wants to overload `Pretty.width`
pub const WIDTH : usize = 80;

/// A pretty-printable type.
///
/// Pretty printing is done via `pp`, which mutates the context
/// passed as an argument.
pub trait Pretty {
    /// Pretty print itself into the given context
    fn pp_into(&self, ctx: &mut Ctx);

    /// Width for printing. Default is `WIDTH`
    fn width(&self) -> usize { WIDTH }

    /// Automatic display into a formatter. This can be used to implement `Debug` or `Display`.
    fn pp_fmt(&self, out: &mut fmt::Formatter, alternate: bool) -> fmt::Result {
        let mut ctx = Ctx::new();
        if alternate { ctx.set_alternate() }
        self.pp_into(&mut ctx);
        let s = ctx.into_str(self.width());
        write!(out, "{}", &s)
    }
}

/// A way to print with `T` as a context.
pub trait Pretty1<T> {
    fn pp1_into(&self, x: &T, ctx: &mut Ctx);

    /// Pretty-printable/debug/display-able version of the given object.
    fn pp<'a>(&'a self, x: &'a T) -> Tmp1<(&'a Self, &'a T)> { Tmp1((self,x)) }
}

/// A way to print with `T` and `U` as context.
pub trait Pretty2<T, U> {
    fn pp2_into(&self, x: &T, y: &U, ctx: &mut Ctx);

    /// Pretty-printable/debug/display-able version of the given object.
    fn pp<'a>(&'a self, x: &'a T, y: &'a U) -> Tmp1<(&'a Self, &'a T, &'a U)> { Tmp1((self,x,y)) }
}

/// Temporary holder of `T`. Can be pretty-printed, displayed,, etc.
pub struct Tmp1<T>(pub T);

impl<'a,T1:Pretty1<T2>,T2> Pretty for Tmp1<(&'a T1,&'a T2)> {
    fn pp_into(&self, ctx: &mut Ctx) { (self.0).0.pp1_into(&(self.0).1, ctx); }
}
impl<'a,T1:Pretty1<T2>,T2> fmt::Debug for Tmp1<(&'a T1,&'a T2)> {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result
    { Pretty::pp_fmt(&self,out,true) }
}
impl<'a,T1:Pretty1<T2>,T2> fmt::Display for Tmp1<(&'a T1,&'a T2)> {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result
    { Pretty::pp_fmt(&self,out,false) }
}

impl<'a,T1:Pretty2<T2,T3>,T2,T3> Pretty for Tmp1<(&'a T1,&'a T2,&'a T3)> {
    fn pp_into(&self, ctx: &mut Ctx) { (self.0).0.pp2_into(&(self.0).1, &(self.0).2, ctx); }
}
impl<'a,T1:Pretty2<T2,T3>,T2,T3> fmt::Debug for Tmp1<(&'a T1,&'a T2,&'a T3)> {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result
    { Pretty::pp_fmt(&self,out,true) }
}
impl<'a,T1:Pretty2<T2,T3>,T2,T3> fmt::Display for Tmp1<(&'a T1,&'a T2,&'a T3)> {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result
    { Pretty::pp_fmt(&self,out,false) }
}

pub fn pp1<'a,T:Pretty1<U>,U>(x:&'a T, y: &'a U) -> impl 'a+Pretty+fmt::Display+fmt::Debug { Tmp1((x,y)) }
pub fn pp2<'a,T:Pretty2<U,V>,U,V>(x:&'a T, y: &'a U, z: &'a V) -> impl 'a+Pretty+fmt::Display+fmt::Debug { Tmp1((x,y,z)) }

/// An array of `T`
pub struct Arr<'a, T>(&'a [T]);

pub fn sexp1<'a,T,M>(x: &'a [T]) -> impl Pretty1<M> + 'a where T: Pretty1<M> {
    Arr(x)
}

// render array in a S-expr
impl<'a, T, M> Pretty1<M> for Arr<'a, T> where T : Pretty1<M> {
    fn pp1_into(&self, m: &M, ctx: &mut Ctx) {
        ctx.sexp(|ctx| {
            for (i,x) in self.0.iter().enumerate() {
                if i > 0 { space().pp_into(ctx); }
                x.pp1_into(m, ctx)
            }
        });
    }
}

// ability to use `Op` directly as a printable object
impl Pretty for Op {
    fn pp_into(&self, ctx: &mut Ctx) { ctx.push_(self.clone()); }
}

/// Display a newline
pub fn newline() -> impl Pretty { Op::Newline }

/// Display a space (or break)
pub fn space() -> impl Pretty { Op::Space }

/// Display a static string
pub fn str(s: &'static str) -> impl Pretty { Op::SStatic(s) }

/// Display a dynamic (owned) string
pub fn string(s: String) -> impl Pretty { Op::Text(s) }

/// Display a dynamic (owned) string
pub fn text<U:Into<String>>(u: U) -> impl Pretty { Op::Text(u.into()) }

struct Sexp<'a>(&'a [&'a dyn Pretty]);
impl<'a> Pretty for Sexp<'a> {
    fn pp_into(&self, ctx: &mut Ctx) {
        ctx.sexp(|ctx| {
            for (i,t) in self.0.iter().enumerate() {
                if i > 0 { ctx.space(); }
                (*t).pp_into(ctx)
            }
        });
    }
}

/// Print the given arguments as a S-expression
pub fn sexp_slice<'a>(v: &'a[&'a dyn Pretty]) -> impl Pretty + 'a { Sexp(v) }

impl<T> Pretty for Tmp<Vec<T>> where T: Pretty {
    fn pp_into(&self, ctx: &mut Ctx) {
        ctx.sexp(|ctx| { ctx.array(space(), &self.0[..]); });
    }
}

/// Print the given arguments as a S-expression
pub fn sexp_iter<I,T>(i: I) -> impl Pretty where I: Iterator<Item=T>, T: Pretty {
    let v: Vec<T> = i.collect();
    Tmp(v)
}

impl<A:Pretty,B:Pretty> Pretty for (A,B) {
    fn pp_into(&self, ctx: &mut Ctx) { ctx.pp(&self.0).pp(&self.1); }
}

impl<A:Pretty,B:Pretty,C:Pretty> Pretty for (A,B,C) {
    fn pp_into(&self, ctx: &mut Ctx) { ctx.pp(&self.0).pp(&self.1).pp(&self.2); }
}

/// Print `a` then `b`
pub fn pair<A,B>(a: A, b: B) -> impl Pretty where A : Pretty, B : Pretty { (a, b) }

/// Print `a` then `b` then `c`
pub fn triple<A,B,C>(a: A, b: B, c: C) -> impl Pretty
    where A : Pretty, B : Pretty, C : Pretty
{ (a,b,c) }

impl<'a, T: Pretty> Pretty for &'a T {
    fn pp_into(&self, ctx: &mut Ctx) { (*self).pp_into(ctx) }
}

/// Print `T` using its Debug implementation
pub fn from_debug<T:fmt::Debug>(x: T) -> impl Pretty {
    struct Dbg<T:fmt::Debug>(T);
    impl<T:fmt::Debug> Pretty for Dbg<T> {
        fn pp_into(&self, ctx: &mut Ctx) { ctx.string(format!("{:?}", self.0)); }
    }
    Dbg(x)
}

/// Alias to `from_debug`
pub fn dbg<T:fmt::Debug>(x: T) -> impl Pretty { from_debug(x) }

/// Print `T` using its Display implementation
pub fn from_display<T:fmt::Display>(x: T) -> impl Pretty {
    struct Dis<T:fmt::Display>(T);
    impl<T:fmt::Display> Pretty for Dis<T> {
        fn pp_into(&self, ctx: &mut Ctx) { ctx.string(format!("{}", self.0)); }
    }
    Dis(x)
}

/// Temporary holder of `T`.
struct Tmp<T>(T);

impl<T:Pretty> fmt::Display for Tmp<T> {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result
    { Pretty::pp_fmt(&self.0,out,false) }
}
impl<T:Pretty> fmt::Debug for Tmp<T> {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result
    { Pretty::pp_fmt(&self.0,out,true) }
}

/// Turn a pretty-printable object into a display-able one
pub fn display<T:Pretty>(x: T) -> impl fmt::Display { Tmp(x) }

/// Turn a pretty-printable object into a debug-able one
pub fn debug<T:Pretty>(x: T) -> impl fmt::Debug { Tmp(x) }

/// Print arrays as S-expressions
impl<T> Pretty for [T] where T : Pretty {
    fn pp_into(&self, ctx: &mut Ctx) {
        ctx.sexp(|ctx| { ctx.array(" ", &self); });
    }
}

/// Make a s-expression from the given objects (which mustt be convertible to Pretty)
#[macro_export]
macro_rules! sexp {
    ($( $t:expr ),* ) => {
        sexp_slice(&[ $( $t ),* ])
    }
}

impl<T> Pretty for Vec<T> where T : Pretty {
    fn pp_into(&self, ctx: &mut Ctx) { self.as_slice().pp_into(ctx) }
}

// Implementations

impl<'a> Pretty for &'a str {
    fn pp_into(&self, ctx: &mut Ctx) { ctx.string(self.to_string()); }
}
impl Pretty for String {
    fn pp_into(&self, ctx: &mut Ctx) { ctx.string(self.clone()); }
}

impl Pretty for std::rc::Rc<str> {
    fn pp_into(&self, out: &mut Ctx) { out.string(self.to_string()); }
}

#[test]
fn test_display() {
    #[derive(Copy,Clone)]
    struct Foo(u32);

    impl Pretty for Foo {
        fn pp_into(&self, ctx: &mut Ctx) { ctx.string(self.0.to_string()); }
    };

    let foo = Foo(42);
    let s = format!("{}", display(&foo));
    assert_eq!("42", s);

    struct V<T>(Vec<T>);
    impl<T:Pretty> Pretty for V<T> {
        fn pp_into(&self, ctx: &mut Ctx) { self.0.pp_into(ctx) }
    };

    let s2 = format!("{}", display(V(vec![Foo(1), Foo(23), Foo(105)])));
    assert_eq!("(1 23 105)", s2);

    let s3 = format!("{}", display(sexp!(&Foo(1), &Foo(23), &Foo(105))));
    assert_eq!("(1 23 105)", s3);
}
