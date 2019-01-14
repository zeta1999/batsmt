
//! Theory built on the congruence closure

use {
    batsmt_core::{ast, backtrack, },
    batsmt_theory as theory,
    batsmt_pretty as pp,
    crate::{CCInterface, CCView, Ctx, pp_t},
};

#[allow(unused_imports)]
use crate::{naive_cc::NaiveCC,cc::CC};

// TODO: notion of micro theory should come here

#[cfg(feature="naive")]
type CCI<M> = NaiveCC<M>;

#[cfg(not(feature="naive"))]
type CCI<M> = CC<M>;

/// A theory built on top of a congruence closure.
#[repr(transparent)]
pub struct CCTheory<C:Ctx>{
    cc: CCI<C>,
}

impl<C:Ctx> CCTheory<C> {
    /// Build a new theory for equality, based on congruence closure.
    pub fn new(m: &mut C) -> Self {
        let cc = CCI::new(m);
        debug!("use {}", CCI::<C>::impl_descr());
        Self { cc }
    }

    /// Add trail to the congruence closure, returns `true` if anything was added
    fn add_trail_to_cc(&mut self, m: &C, trail: &theory::Trail<C>) -> bool {
        let mut done_sth = false;

        // update congruence closure
        for (ast,sign,lit) in trail.iter() {
            // convert `ast is {true,false}` into a merge/distinct op
            match m.view_as_cc_term(&ast) {
                CCView::Eq(a,b) => {
                    if sign {
                        self.cc.merge(m, *a, *b, lit);
                    } else {
                        // `(a=b)=false`
                        self.cc.merge(m, ast, m.get_bool_term(false), lit);
                    }
                },
                CCView::Distinct(args) => {
                    if !sign {
                        panic!("cannot handle negative `distinct`")
                    };
                    self.cc.distinct(m, args, lit)
                },
                _ if sign => {
                    self.cc.merge(m, ast, m.get_bool_term(true), lit)
                },
                _ => {
                    self.cc.merge(m, ast, m.get_bool_term(false), lit)
                }
            }

            done_sth = true;
        }
        done_sth
    }
}

impl<C:Ctx> backtrack::Backtrackable<C> for CCTheory<C> {
    fn push_level(&mut self, c: &mut C) { self.cc.push_level(c) }
    fn pop_levels(&mut self, c: &mut C, n:usize) { self.cc.pop_levels(c, n) }
    fn n_levels(&self) -> usize { self.cc.n_levels() }
}

impl<C:Ctx> theory::Theory<C> for CCTheory<C> {
    fn final_check<A>(
        &mut self, ctx: &mut C,
        acts: &mut A, trail: &theory::Trail<C>
    ) where A: theory::Actions<C>
    {
        debug!("cc.final-check");
        self.add_trail_to_cc(ctx, trail);
        self.cc.final_check(ctx, acts);
    }

    fn partial_check<A>(
        &mut self, ctx: &mut C,
        acts: &mut A, trail: &theory::Trail<C>
    ) where A: theory::Actions<C>
    {
        if ! CCI::<C>::has_partial_check() {
            return; // doesn't handle partial checks
        }
        debug!("cc.partial-check");
        trace!("trail: {:?}", trail.as_slice());

        // TODO: shouldn't this shortcut be done in main solver already?
        let do_sth = self.add_trail_to_cc(ctx, trail);
        if !do_sth {
            return; // nothing new
        }
        self.cc.partial_check(ctx, acts);
    }

    #[inline(always)]
    fn has_partial_check() -> bool {
        CCI::<C>::has_partial_check()
    }

    #[inline]
    fn add_literal(&mut self, ctx: &C, t: C::AST, lit: C::B) {
        self.cc.add_literal(ctx, t,lit);
    }

    #[inline]
    fn explain_propagation(&mut self, m: &C, _t: C::AST, _sign: bool, p: C::B) -> &[C::B] {
        // what does `t=sign` correspond to?
        trace!("explain-prop {} sign={} (lit {:?})", pp_t(m,&_t), _sign, p);
        self.cc.explain_prop(m, p)
    }
}
