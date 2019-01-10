
//! Congruence closure.
//!
//! The `CC` type is responsible for enforcing congruence and transitivity
//! of equality.

// TODO: parametrize by "micro theories" (which just work on merges + expl)

// micro theories:
// - rule for `not` (t == true |- not t == false, and conversely)
// - rule for `ite` (t == true |- not t == false, and conversely)

// TODO(perf): backtrackable array allocator for signatures
// TODO(perf): sparse map for nodes (using a second dense array)?

use {
    std::{ u32, ptr, hash::Hash, fmt::Debug, },
    batsmt_core::{ast::{self,AstMap,DenseMap,View}, backtrack, ast_u32, },
    batsmt_theory::BoolLit,
    batsmt_pretty::{self as pp, Pretty1},
    crate::{
        types::{Ctx, Actions }, Builtins, CCInterface, SVec,
    },
};

#[repr(u8)]
enum TraverseTask { Enter, Exit }

/// The congruence closure.
pub struct CC<C:Ctx> {
    b: Builtins<C::AST>,
    pending: Vec<C::AST>, // update signatures
    combine: Vec<(C::AST,C::AST,Expl<C::AST,C::B>)>, // merge
    undo: backtrack::Stack<UndoOp<C::AST>>,
    props: Vec<C::B>, // local for propagations
    expl_st: Vec<Expl<C::AST, C::B>>, // to expand explanations
    tmp_sig: Signature<C::AST>, // for computing signatures
    traverse: Vec<(TraverseTask,C::AST)>, // for adding terms
    sig_tbl: backtrack::HashMap<Signature<C::AST>, C::AST>,
    lit_expl: backtrack::HashMap<C::B, LitExpl<C::AST,C::B>>, // pre-explanations for propagations
    cc1: CC1<C>,
}

/// internal state, with just the core structure for nodes and parent sets
struct CC1<C:Ctx> {
    ok: bool, // no conflict?
    alloc_parent_list: ListAlloc<C::AST>,
    nodes: Nodes<C>,
    confl: Vec<C::B>, // local for conflict
}

/// Explanation for the propagation of a boolean literal.
#[derive(Debug)]
enum LitExpl<AST,B> {
    Propagated {
        // we propagated it from given merges
        eqns: [(AST,AST); 2],
        expl: Expl<AST,B>,
    },
}

type DMap<B> = ast_u32::DenseMap<Node<ast_u32::AST, B>>;

/// Map terms into the corresponding node.
struct Nodes<C:Ctx>{
    map: DMap<C::B>,
    find_stack: Vec<C::AST>,
}

/// One node in the congruence closure's E-graph.
///
/// It contains:
/// - the term itself
/// - a direct pointer to the class' root
/// - pointer to next element of the class (circular list)
/// - a linked list of parent terms
/// - the proof forest pointer
/// - an optional mapping from the term to a boolean literal (for propagation)
#[derive(Clone)]
struct Node<AST, B> where AST : Sized, B : Sized {
    id: AST,
    next: AST, // next elt in class
    expl: Option<(AST, Expl<AST,B>)>, // proof forest
    as_lit: Option<B>, // if this term corresponds to a boolean literal
    root: Repr<AST>, // current representative (initially, itself)
    parents: List<AST>,
}

/// A node in a singly-linked list of objects.
#[derive(Copy,Clone)]
struct ListC<T>(T, *mut ListC<T>);

type ListAlloc<T> = backtrack::Alloc<ListC<T>>;

/// A singly-linked list of T.
#[derive(Clone)]
struct List<T> {
    first: *mut ListC<T>, // first node (null iff len==0)
    last: *mut ListC<T>, // last node (null iff len==0)
    len: u32, // number of nodes
}

/// A representative
#[derive(Debug,Clone,Copy,Eq,PartialEq,Hash,Ord,PartialOrd)]
struct Repr<AST>(AST);

/// An explanation for a merge
#[derive(Clone,Debug)]
enum Expl<AST, B> {
    Lit(B), // because literal was asserted
    Congruence(AST, AST), // because terms are congruent
    AreEq(AST, AST), // because terms are equal
}

/// Undo operations on the congruence closure
#[derive(Debug)]
enum UndoOp<AST> {
    SetOk,
    RemoveTerm(AST),
    UnmapLit(AST),
    Unmerge {
        root: Repr<AST>, // the new repr
        old_root: Repr<AST>, // merged into `a`
    }, // unmerge these two reprs
    RemoveExplLink(AST,AST), // remove explanation link connecting these
}

/// A signature for a term, obtained by replacing its subterms with their repr.
///
/// Signatures have the properties that if two terms are congruent,
/// then their signatures are identical.
#[derive(Eq,PartialEq,Hash,Clone,Debug)]
struct Signature<AST:Eq+Hash+Debug>(SVec<Repr<AST>>);

// implement main interface
impl<C:Ctx> CCInterface<C> for CC<C> {
    fn merge(&mut self, m: &C, t1: C::AST, t2: C::AST, lit: C::B) {
        debug!("merge {} and {} (expl {:?})", ast::pp(m,&t1), ast::pp(m,&t2), lit);
        self.add_term(m, t1);
        self.add_term(m, t2);
        let expl = Expl::Lit(lit);
        self.combine.push((t1,t2,expl));
    }

    fn distinct(&mut self, _m: &C, _ts: &[C::AST], _lit: C::B) {
        unimplemented!("distinct is not supported yet")
        /*
        let mut v = SVec::with_capacity(ts.len());
        v.extend_from_slice(ts);
        let expl = Expl::Lit(lit);
        self.tasks.push_back(Task::Distinct(v,expl))
        */
    }

    fn add_literal(&mut self, m: &C, t: C::AST, lit: C::B) {
        self.add_term(m, t);
        self.map_to_lit(m, t, lit);
    }

    #[inline]
    fn final_check<A>(&mut self, m: &C, acts: &mut A)
        where A: Actions<C>
    {
        self.check_internal(m, acts)
    }

    #[inline]
    fn partial_check<A>(&mut self, m: &C, acts: &mut A)
        where A: Actions<C>
    {
        self.check_internal(m, acts)
    }

    fn explain_prop(&mut self, m: &C, p: C::B) -> &[C::B] {
        trace!("explain prop {:?}", p);
        let e = self.lit_expl.get(&p).expect("no explanation recorded");
        trace!("pre-explanation is {:?}", e);
        match e {
            LitExpl::Propagated{eqns,expl} => {
                let mut er = ExplResolve::new(&mut self.cc1, &mut self.expl_st);
                er.add_expl(expl.clone());
                for (a,b) in eqns.iter() {
                    er.explain_eq(m, *a, *b)
                }
                er.fixpoint(m);
                trace!("... final explanation: {:?}", &self.cc1.confl[..]);
                &self.cc1.confl[..]
            },
        }
    }

    fn has_partial_check() -> bool { true }

    fn impl_descr() -> &'static str { "fast congruence closure"}
}

impl<C:Ctx> CC<C> {
    // main `check` function, performs the fixpoint
    fn check_internal<A>(&mut self, m: &C, acts: &mut A)
        where A: Actions<C>
    {
        debug!("check-internal (pending: {}, combine: {})",
            self.pending.len(), self.combine.len());
        self.fixpoint(m);
        if self.cc1.ok {
            for &p in &self.props {
                acts.propagate(p)
            }
        } else {
            debug_assert!(self.cc1.confl.len() >= 1); // must have some conflict
            let costly = true;
            acts.raise_conflict(&self.cc1.confl, costly)
        }
        self.props.clear();
    }

    /// Main CC algorithm.
    fn fixpoint(&mut self, m: &C) {
        let CC{combine,cc1,b,pending,expl_st,undo,tmp_sig,sig_tbl,props,lit_expl,..} = self;
        loop {
            if !cc1.ok {
                combine.clear();
                pending.clear();
                break
            }

            {
                let mut updsig = UpdateSigPhase{cc1,combine,b,sig_tbl,tmp_sig};
                for &t in pending.iter() {
                    updsig.update_signature(m, t);
                }
                pending.clear();
            }

            {
                let mut merger = MergePhase{cc1,b,pending,expl_st,undo,props,lit_expl};
                for (t,u,expl) in combine.iter() {
                    merger.merge(m,*t,*u,expl.clone())
                }
                combine.clear();
            }

            if pending.is_empty() {
                break; // done
            }
        }
    }
}

// main congruence closure operations
impl<C:Ctx> CC<C> {
    /// Create a new congruence closure.
    pub fn new(_m: &mut C, b: Builtins<C::AST>) -> Self {
        let map = ast_u32::DenseMap::new(Node::sentinel());
        let mut cc1 = CC1::new(map);
        // add builtins
        cc1.nodes.insert(b.true_, Node::new(b.true_));
        cc1.nodes.insert(b.false_, Node::new(b.false_));
        CC{
            b,
            pending: vec!(),
            combine: vec!(),
            props: vec!(),
            undo: backtrack::Stack::new(),
            traverse: vec!(),
            tmp_sig: Signature::new(),
            sig_tbl: backtrack::HashMap::new(),
            expl_st: vec!(),
            lit_expl: backtrack::HashMap::new(),
            cc1,
        }
    }

    /// Add this term to the congruence closure, if not present already.
    fn add_term(&mut self, m: &C, t0: C::AST) {
        if self.cc1.nodes.contains(&t0) {
            return; // fast path: already there
        }

        self.traverse.clear();

        let CC {traverse, undo, cc1, pending, ..} = self;
        // traverse in postfix order (shared context: `cc1`)

        traverse.push((TraverseTask::Enter, t0));
        while let Some((task,t)) = traverse.pop() {
            match task {
                TraverseTask::Enter => {
                    if ! cc1.nodes.contains(&t) {
                        cc1.nodes.insert(t, Node::new(t));

                        traverse.push((TraverseTask::Exit, t));
                        // add subterms
                        for u in m.view(&t).subterms() {
                            traverse.push((TraverseTask::Enter,*u))
                        }
                    }
                },
                TraverseTask::Exit => {
                    // now add itself to its children's list of parents.
                    for &u in m.view(&t).subterms() {
                        debug_assert!(cc1.nodes.contains(&u)); // postfix order
                        let ur = cc1.find(u);
                        let parents = cc1.nodes.parents_mut(ur.0);
                        parents.add(&mut cc1.alloc_parent_list, t);
                    }
                    // remove `t` before its children
                    undo.push_if_nonzero(UndoOp::RemoveTerm(t));
                    pending.push(t)
                },
            }
        }
    }

    fn map_to_lit(&mut self, m: &C, t: C::AST, lit: C::B) {
        if ! self.cc1.ok {
            return;
        }

        debug_assert!(self.cc1.nodes.contains(&t));
        let n = &mut self.cc1[t];
        if n.as_lit.is_none() {
            debug!("map term {} to literal {:?}", ast::pp(m,&t), lit);
            n.as_lit = Some(lit);
            self.undo.push_if_nonzero(UndoOp::UnmapLit(t));
        }
    }
}

struct MergePhase<'a,C:Ctx> {
    cc1: &'a mut CC1<C>,
    b: &'a Builtins<C::AST>,
    pending: &'a mut Vec<C::AST>,
    expl_st: &'a mut Vec<Expl<C::AST, C::B>>,
    undo: &'a mut backtrack::Stack<UndoOp<C::AST>>,
    props: &'a mut Vec<C::B>, // local for propagations
    lit_expl: &'a mut backtrack::HashMap<C::B, LitExpl<C::AST,C::B>>, // pre-explanations for propagations
}

struct UpdateSigPhase<'a,C:Ctx> {
    cc1: &'a mut CC1<C>,
    b: &'a Builtins<C::AST>,
    combine: &'a mut Vec<(C::AST,C::AST,Expl<C::AST,C::B>)>,
    sig_tbl: &'a mut backtrack::HashMap<Signature<C::AST>, C::AST>,
    tmp_sig: &'a mut Signature<C::AST>,
}

impl<'a, C:Ctx> MergePhase<'a,C> {
    /// Merge `a` and `b`, if they're not already equal.
    fn merge(&mut self, m: &C, mut a: C::AST, mut b: C::AST, expl: Expl<C::AST, C::B>) {
        debug_assert!(self.cc1.contains_ast(a));
        debug_assert!(self.cc1.contains_ast(b));

        let mut ra = self.cc1.nodes.find(a);
        let mut rb = self.cc1.nodes.find(b);
        debug_assert!(self.cc1.is_root(ra.0), "{}.repr = {}", ast::pp(m,&a), ast::pp(m,&ra.0));
        debug_assert!(self.cc1.is_root(rb.0), "{}.repr = {}", ast::pp(m,&b), ast::pp(m,&rb.0));

        if ra == rb {
            return; // done already
        }

        // access the two nodes
        let (na, nb) = self.cc1.nodes.map.get2(ra.0, rb.0);

        // NOTE: would be nice to put `unlikely` there once it stabilizes
        if na.len() + nb.len() > u32::MAX as usize {
            panic!("too many terms in one class")
        }

        // merge smaller one into bigger one, except that booleans are
        // always representatives
        if rb.0 == self.b.true_ || rb.0 == self.b.false_ {
            // conflict: merge true with false (since they are distinct)
            if ra.0 == self.b.true_ || ra.0 == self.b.false_ {
                // generate conflict from `expl`, `a == ra`, `b == rb`
                trace!("generate conflict from merge of true/false from {} and {}",
                       ast::pp(m,&a), ast::pp(m,&b));
                self.cc1.ok = false;
                self.undo.push_if_nonzero(UndoOp::SetOk);
                {
                    let mut er = ExplResolve::new(&mut self.cc1, &mut self.expl_st);
                    er.add_expl(expl);
                    er.explain_eq(m, a, ra.0);
                    er.explain_eq(m, b, rb.0);
                    er.fixpoint(m);
                }
                // negation
                for lit in self.cc1.confl.iter_mut() { *lit = ! *lit }
                trace!("inconsistent set of explanations: {:?}", &self.cc1.confl);
                return;
            } else {
                // merge into true/false
                std::mem::swap(&mut ra, &mut rb);
            }
        } else if ra.0 == self.b.true_ || ra.0 == self.b.false_ {
            // `ra` must be repr
        } else if na.len() < nb.len() {
            // swap a and b
            std::mem::swap(&mut ra, &mut rb);
            std::mem::swap(&mut a, &mut b);
        }
        drop(na);
        drop(nb);

        trace!("merge {} into {}", ast::pp(m,&rb.0), ast::pp(m,&ra.0));

        // update forest tree so that `b --[expl]--> a`.
        // Note that here we link `a` and `b`, not their representatives.
        {
            self.undo.push_if_nonzero(UndoOp::RemoveExplLink(a,b));

            self.cc1.reroot_forest(m, b);
            let nb = &mut self.cc1[b];
            debug_assert!(nb.expl.is_none());
            nb.expl = Some((a, expl.clone()));
        }

        // undo the merge on backtrack
        self.undo.push_if_nonzero(UndoOp::Unmerge {
            root: ra, old_root: rb,
        });

        // might need to check for congruence again for parents of `b`,
        // since one of their direct child (b) now has a different
        // representative
        {
            let MergePhase{cc1, pending, ..} = self;
            cc1.nodes.iter_parents(rb, |p_b| {
                pending.push(*p_b)
            });
        }

        let MergePhase{cc1, b: bs, props, lit_expl, ..} = self;

        // if ra is {true,false}, propagate lits
        if ra.0 == bs.true_ || ra.0 == bs.false_ {
            trace!("{}.class: look for propagations", ast::pp(m,&rb.0));
            cc1.nodes.iter_class(rb, |n| {
                trace!("... {}.iter_class: check {}", ast::pp(m,&rb.0), ast::pp(m,&n.id));
                n.root = ra; // while we're there, we can merge eagerly.

                // if a=true/false, and `n` has a literal, propagate the literal
                // assuming it's not known to be true already.
                if let Some(mut lit) = n.as_lit {
                    if !lit_expl.contains_key(&lit) {
                        // future expl: `n=b=a=ra` (where ra∈{true,false})
                        let e = LitExpl::Propagated {
                            expl: expl.clone(),
                            eqns:[(n.id, b), (a, ra.0)],
                        };
                        if ra.0 == bs.true_ {
                            trace!("propagate literal {:?} (for true≡{}, {:?})",
                                lit, ast::pp(m,&n.id), &expl);
                            props.push(lit);
                        } else {
                            debug_assert_eq!(ra.0, bs.false_);
                            lit = !lit;
                            trace!("propagate literal {:?} (for false≡{}, {:?})",
                                lit, ast::pp(m,&n.id), &expl);
                            props.push(lit);
                        }
                        lit_expl.insert(lit, e);
                    }
                }
            });
        }

        // set `rb.root` to `ra`
        cc1[rb.0].root = ra;

        // ye ole' switcharoo (of doubly linked lists for the equiv class)
        //
        // instead of:  a --> next_a,  b --> next_b
        // have:  a --> next_b, b --> next_a
        //
        {
            // access the two nodes, again
            let (na, nb) = self.cc1.nodes.map.get2(ra.0, rb.0);

            let next_a = na.next;
            let next_b = nb.next;

            na.next = next_b;
            nb.next = next_a;

            // also merge parent lists
            na.parents.append(&mut nb.parents)
        }
    }
}

impl<'a, C:Ctx> UpdateSigPhase<'a,C> {
    /// Check and update signature of `t`, possibly adding new merged by congruence.
    fn update_signature(&mut self, m: &C, t: C::AST) {
        let UpdateSigPhase{tmp_sig: ref mut sig, ref mut cc1, sig_tbl, combine, b: bs, ..} = self;
        match m.view(&t) {
            View::Const(_) => (),
            View::App {f, args} if f == bs.eq => {
                // do not compute a signature, but check if `args[0]==args[1]`
                let a = args[0];
                let b = args[1];
                if self.cc1.nodes.is_eq(a,b) {
                    trace!("merge {} with true by eq-rule", ast::pp(m,&t));
                    let expl = Expl::AreEq(a,b);
                    combine.push((t, bs.true_, expl))
                }
            },
            View::App {f, args} => {
                // compute signature and look for collisions
                sig.compute_app(cc1, &f, args);
                match sig_tbl.get(sig) {
                    None => {
                        // insert into signature table
                        sig_tbl.insert(sig.clone(), t);
                    },
                    Some(u) if t == *u => (),
                    Some(u) => {
                        // collision, merge `t` and `u` as they are congruent
                        trace!("merge by congruence: {} and {}", ast::pp(m,&t), ast::pp(m,u));
                        let expl = Expl::Congruence(t, *u);
                        combine.push((t, *u, expl))
                    }
                }
            },
        }
    }
}

// query the graph
impl<C:Ctx> CC1<C> {
    fn new(map: ast_u32::DenseMap<Node<C::AST, C::B>>) -> Self {
        CC1 {
            nodes: Nodes::new(map),
            ok: true,
            alloc_parent_list: backtrack::Alloc::new(),
            confl: vec!(),
        }
    }

    #[inline(always)]
    fn contains_ast(&self, t: C::AST) -> bool { self.nodes.contains(&t) }

    #[inline(always)]
    fn find(&mut self, t: C::AST) -> Repr<C::AST> { self.nodes.find(t) }

    #[inline(always)]
    fn is_root(&mut self, id: C::AST) -> bool { self.find(id).0 == id }

    /// Undo one change.
    fn perform_undo(&mut self, m: &C, op: UndoOp<C::AST>) {
        trace!("perform-undo {}", op.pp(m));
        match op {
            UndoOp::SetOk => {
                self.ok = true;
                self.confl.clear();
            },
            UndoOp::Unmerge {root: a, old_root: b} => {
                assert_ne!(a.0,b.0); // crucial invariant

                let (na, nb) = self.nodes.map.get2(a.0, b.0);

                // inverse switcharoo for the class linked lists
                {
                    let next_a = na.next;
                    let next_b = nb.next;

                    na.next = next_b;
                    nb.next = next_a;

                    na.parents.un_append(&mut nb.parents);
                }

                // reset `root` pointer for `nb`
                // reset root for the classes of terms that are now repr again,
                // if they haven't been removed.
                self.nodes.iter_class(b, |nb1| {
                    nb1.root = b;
                });
            },
            UndoOp::RemoveExplLink(a,b) => {
                // one of {a,b} points to the other, explanation wise.
                // Be sure to remove this link from the proof forest.
                {
                    assert_ne!(a,b);
                    let (na, nb) = self.nodes.map.get2(a,b);
                    match nb.expl {
                        Some((ra2,_)) if ra2 == a => nb.expl = None,
                        _ => ()
                    }
                    match na.expl {
                        Some((rb2,_)) if rb2 == b => na.expl = None,
                        _ => ()
                    }
                }
            },
            UndoOp::RemoveTerm(t) => {
                debug_assert_eq!(0, self.nodes[t].parents.len(), "remove term with parents");
                self.nodes.remove(&t);

                // remove from children's parents' lists
                for &u in m.view(&t).subterms() {
                    let ur = self.find(u);
                    let parents = self.nodes.parents_mut(ur.0);
                    let _t = parents.remove();
                    debug_assert_eq!(_t, t);
                }
            }
            UndoOp::UnmapLit(t) => {
                let n = &mut self[t];
                debug_assert!(n.as_lit.is_some());
                n.as_lit = None;
            },
        }
    }

    /// Reroot proof forest for the class of `r` so that `r` is the root.
    fn reroot_forest(&mut self, m: &C, t: C::AST) {
        trace!("reroot-forest-to {}", ast::pp(m,&t));
        let (mut cur_t, mut expl) = {
            let n = &mut self[t];
            match &n.expl {
                None => {
                    return; // rooted in `t` already
                },
                Some((u,e)) => {
                    let tup = (*u,e.clone()).clone();
                    n.expl = None;
                    tup
                }
            }
        };

        let mut prev_t = t;
        loop {
            let cur_node = &mut self[cur_t];
            let mut expl_tup = Some((prev_t,expl));
            // set `cur_node.expl = (prev_t, expl)`
            std::mem::swap(&mut cur_node.expl, &mut expl_tup);
            // `expl_tup` now has `cur_node.expl`'s former value.
            match expl_tup {
                None => break,
                Some((next, expl_next)) => {
                    // follow pointer
                    prev_t = cur_t;
                    cur_t = next;
                    expl = expl_next;
                },
            }
        }
    }

    /// Distance separating `t` from the root of its explanation forest.
    fn dist_to_expl_root(&self, mut t: C::AST) -> usize {
        let mut dist = 0;

        while let Node {expl: Some((next_t,_)), ..} = self[t] {
            dist += 1;
            t = next_t;
        }
        dist
    }

    /// Find the closest common ancestor of `a` and `b` in the proof
    /// forest.
    /// Precond: `is_eq(a,b)`.
    fn find_expl_common_ancestor(&mut self, mut a: C::AST, mut b: C::AST) -> C::AST {
        debug_assert!(self.nodes.is_eq(a,b));

        let dist_a = self.dist_to_expl_root(a);
        let dist_b = self.dist_to_expl_root(b);
        if dist_a > dist_b {
            a = self.skip_expl_links(a, dist_a - dist_b);
        } else if dist_a < dist_b {
            b = self.skip_expl_links(b, dist_b - dist_a);
        };

        // now walk in lock-step until we find the ancestor
        loop {
            if a == b {
                return a;
            }

            if let Some((a2,_)) = self[a].expl { a = a2 } else { panic!() };
            if let Some((b2,_)) = self[b].expl { b = b2 } else { panic!() };
        }
    }

    /// Skip `n` links in the explanation forest.
    fn skip_expl_links(&self, mut t: C::AST, mut n: usize) -> C::AST {
        while n > 0 {
            if let Some((t2,_)) = self[t].expl { t = t2 } else { panic!() };
            n -= 1
        };
        t
    }
}

impl<C:Ctx> backtrack::Backtrackable<C> for CC<C> {
    fn push_level(&mut self, m: &mut C) {
        trace!("push-level");
        self.fixpoint(m); // be sure to commit changes before saving
        self.undo.push_level();
        self.sig_tbl.push_level();
        self.cc1.alloc_parent_list.push_level();
        self.lit_expl.push_level();
    }

    fn pop_levels(&mut self, m: &mut C, n: usize) {
        debug_assert_eq!(self.undo.n_levels(), self.sig_tbl.n_levels());
        debug_assert_eq!(self.undo.n_levels(), self.cc1.alloc_parent_list.n_levels());
        if n > 0 {
            trace!("pop-levels {}", n);
            let cc1 = &mut self.cc1;
            self.undo.pop_levels(n, |op| cc1.perform_undo(m, op));
            self.sig_tbl.pop_levels(n);
            cc1.alloc_parent_list.pop_levels(n);
            self.lit_expl.pop_levels(n);
            self.props.clear();
            self.pending.clear();
            self.combine.clear();
        }
    }

    #[inline(always)]
    fn n_levels(&self) -> usize { self.undo.n_levels() }
}

/// Temporary structure to resolve explanations.
struct ExplResolve<'a,C:Ctx> {
    cc1: &'a mut CC1<C>,
    expl_st: &'a mut Vec<Expl<C::AST, C::B>>,
}

impl<'a,C:Ctx> ExplResolve<'a,C> {
    /// Create the temporary structure.
    fn new(cc1: &'a mut CC1<C>, expl_st: &'a mut Vec<Expl<C::AST,C::B>>) -> Self {
        expl_st.clear();
        cc1.confl.clear();
        ExplResolve { cc1, expl_st }
    }

    fn add_expl(&mut self, e: Expl<C::AST, C::B>) {
        self.expl_st.push(e)
    }

    /// Main loop for turning explanations into a conflict.
    ///
    /// Pushes the resulting literals into `self.cc1.confl`.
    fn fixpoint(mut self, m: &C) -> &'a Vec<C::B> {
        while let Some(e) = self.expl_st.pop() {
            trace!("expand-expl: {}", e.pp(m));
            match e {
                Expl::Lit(lit) => {
                    self.cc1.confl.push(lit);
                },
                Expl::AreEq(a,b) => {
                    self.explain_eq(m, a, b);
                },
                Expl::Congruence(a,b) => {
                    // explain why arguments are pairwise equal
                    match (m.view(&a), m.view(&b)) {
                        (View::App {f: f1, args: args1},
                         View::App {f: f2, args: args2}) => {
                            debug_assert_eq!(args1.len(), args2.len());
                            self.explain_eq(m, f1, f2);
                            for i in 0 .. args1.len() {
                                self.explain_eq(m, args1[i], args2[i]);
                            }
                        },
                        _ => unreachable!(),
                    }
                }
            }
        }
        // cleanup conflict
        self.cc1.confl.sort_unstable();
        self.cc1.confl.dedup();
        &self.cc1.confl
    }

    /// Explain why `a` and `b` were merged, pushing some sub-tasks
    /// onto `self.expl_st`, and leaf literals onto `self.confl`.
    fn explain_eq(&mut self, m: &C, a: C::AST, b: C::AST) {
        if a == b { return }
        trace!("explain eq of {} and {}", ast::pp(m,&a), ast::pp(m,&b));

        let common_ancestor = self.cc1.find_expl_common_ancestor(a, b);
        trace!("common ancestor: {}", ast::pp(m,&common_ancestor));
        self.explain_along_path(a, common_ancestor);
        self.explain_along_path(b, common_ancestor);
    }

    /// Explain why `cur =_E ancestor`, where `ancestor` is reachable from `cur`
    fn explain_along_path(&mut self, mut cur: C::AST, ancestor: C::AST) {
        while cur != ancestor {
            if let Some((next, expl)) = &self.cc1[cur].expl {
                self.expl_st.push(expl.clone()); // need to explain this link
                cur = *next;
            } else {
                panic!()
            }
        }
    }
}

impl<C:Ctx> Nodes<C> {
    fn new(map: DMap<C::B>) -> Self {
        Nodes{ map, find_stack: vec!() }
    }

    #[inline(always)]
    fn contains(&self, t: &C::AST) -> bool { self.map.contains(t) }

    #[inline(always)]
    fn insert(&mut self, t: C::AST, n: Node<C::AST, C::B>) {
        self.map.insert(t, n)
    }

    /// Find representative of the given node
    #[inline(always)]
    fn find(&mut self, t: C::AST) -> Repr<C::AST> {
        let root = self[t].root;

        if root.0 == t {
            root // fast path
        } else {
            let root = self.find_rec(root.0, 16); // use recursion
            self[t].root = root; // update this pointer
            root
        }
    }

    // find + path compression, using recursion up to `limit`
    fn find_rec(&mut self, t: C::AST, limit: usize) -> Repr<C::AST> {
        let root = self[t].root;
        if root.0 == t {
            root
        } else {
            // recurse
            let root = {
                if limit == 0 {
                    self.find_loop(root.0)
                } else {
                    self.find_rec(root.0, limit-1)
                }
            };
            self[t].root = root;
            root
        }
    }

    // find + path compression, using a loop
    fn find_loop(&mut self, mut t: C::AST) -> Repr<C::AST> {
        debug_assert_eq!(self.find_stack.len(), 0);

        let root = loop {
            let root = self[t].root;
            if t == root.0 {
                break root
            } else {
                // seek further
                self.find_stack.push(t);
                t = root.0
            }
        };

        // update root pointers (path compression)
        for &u in self.find_stack.iter() {
            self.map[u].root = root;
        }
        self.find_stack.clear();

        root
    }

    /// Are these two terms known to be equal?
    #[inline(always)]
    fn is_eq(&mut self, a: C::AST, b: C::AST) -> bool {
        self.find(a) == self.find(b)
    }

    #[inline(always)]
    fn parents_mut(&mut self, t: C::AST) -> &mut List<C::AST> { &mut self[t].parents }

    #[inline(always)]
    fn remove(&mut self, t: &C::AST) {
        self.map.remove(&t)
    }

    /// Call `f` with a mutable ref on all nodes of the class of `r`
    fn iter_class<F>(&mut self, r: Repr<C::AST>, mut f: F)
        where F: for<'b> FnMut(&'b mut Node<C::AST, C::B>)
    {
        let mut t = r.0;
        loop {
            let n = &mut self[t];
            f(n);

            t = n.next;
            if t == r.0 { break } // done the full loop
        }
    }

    /// Call `f` on all parents of all nodes of the class of `r`
    #[inline]
    fn iter_parents<F>(&mut self, r: Repr<C::AST>, f: F)
        where F: FnMut(&C::AST)
    {
        self[r.0].parents.for_each(f);
    }
}

mod cc {
    use super::*;

    impl<C:Ctx> std::ops::Index<C::AST> for Nodes<C> {
        type Output = Node<C::AST, C::B>;
        #[inline(always)]
        fn index(&self, id: C::AST) -> &Self::Output {
            self.map.get_unchecked(&id)
        }
    }

    impl<C:Ctx> std::ops::IndexMut<C::AST> for Nodes<C> {
        #[inline(always)]
        fn index_mut(&mut self, id: C::AST) -> &mut Self::Output {
            self.map.get_mut_unchecked(&id)
        }
    }

    impl<C:Ctx> std::ops::Index<C::AST> for CC1<C> {
        type Output = Node<C::AST, C::B>;
        #[inline(always)]
        fn index(&self, id: C::AST) -> &Self::Output {
            &self.nodes[id]
        }
    }

    impl<C:Ctx> std::ops::IndexMut<C::AST> for CC1<C> {
        #[inline(always)]
        fn index_mut(&mut self, id: C::AST) -> &mut Self::Output {
            &mut self.nodes[id]
        }
    }
}

// manipulating nodes
mod node {
    use super::*;

    impl<AST:Eq, B> PartialEq for Node<AST, B> {
        fn eq(&self, other: &Node<AST, B>) -> bool { self.id == other.id }
    }

    impl<B> Node<ast_u32::AST, B> {
        /// Create a new node
        pub fn new(id: ast_u32::AST) -> Self {
            let parents = List::new();
            Node {
                id, next: id, expl: None,
                root: Repr(id), as_lit: None, parents,
            }
        }

        /// The default `node` object
        #[inline(always)]
        pub(super) fn sentinel() -> Self {
            Node::new(ast_u32::AST::SENTINEL)
        }

        #[inline(always)]
        pub fn len(&self) -> usize {
            self.parents.len()
        }
    }
}

mod expl {
    use super::*;

    impl<C:Ctx> pp::Pretty1<C> for Expl<C::AST, C::B> {
        fn pp_with(&self, m: &C, ctx: &mut pp::Ctx) {
            match self {
                Expl::Lit(lit) => {
                    ctx.str("lit(").debug(lit).str(")");
                },
                Expl::AreEq(a,b) => {
                    ctx.str("are-eq(").pp(&ast::pp(m,a)).str(", ").pp(&ast::pp(m,b)).str(")");
                }
                Expl::Congruence(a,b) => {
                    ctx.str("congruence(").pp(&ast::pp(m,a)).str(", ").pp(&ast::pp(m,b)).str(")");
                }
            }
        }
    }

    impl<C:Ctx> pp::Pretty1<C> for UndoOp<C::AST> {
        fn pp_with(&self, m: &C, ctx: &mut pp::Ctx) {
            match self {
                UndoOp::SetOk => { ctx.str("set-ok"); },
                UndoOp::Unmerge{root:a,old_root:b} => {
                    ctx.str("unmerge(").pp(&ast::pp(m,&a.0)).str(", ").pp(&ast::pp(m,&b.0)).str(")");
                },
                UndoOp::RemoveExplLink(a,b) => {
                    ctx.str("remove-expl-link(").pp(&ast::pp(m,a)).str(", ").pp(&ast::pp(m,b)).str(")");
                },
                UndoOp::UnmapLit(t) => {
                    ctx.str("unmap(").pp(&ast::pp(m,t)).str(")");
                },
                UndoOp::RemoveTerm(t) => {
                    ctx.str("remove-term(").pp(&ast::pp(m,t)).str(")");
                }
            }
        }
    }
}

impl<T:Clone> List<T> {
    /// New list
    #[inline]
    fn new() -> Self {
        List { first: ptr::null_mut(), last: ptr::null_mut(), len: 0 }
    }

    /// Length of the list.
    #[inline(always)]
    fn len(&self) -> usize { self.len as usize }

    /// Iterate over the elements of the list.
    #[inline(always)]
    fn for_each<F>(&self, f: F) where F: FnMut(&T) {
        if self.len > 0 {
            self.for_each_loop(f)
        }
    }

    fn for_each_loop<F>(&self, mut f: F) where F: FnMut(&T) {
        debug_assert!(self.len() > 0);

        let mut ptr = self.first;
        loop {
            let cell = unsafe{ &* ptr };
            f(&cell.0);

            ptr = cell.1;
            if ptr.is_null() {
                break // done
            }
        }
    }

    /// Add `x` into the list.
    fn add(&mut self, alloc: &mut ListAlloc<T>, x: T) {
        // allocate new node
        let ptr = alloc.alloc(ListC(x, self.first));
        self.first = ptr;

        if self.len == 0 {
            // also last element
            self.last = ptr
        }
        self.len += 1;
    }

    /// Remove last added element and return it.
    fn remove(&mut self) -> T {
        debug_assert!(self.len > 0);
        debug_assert!(! self.first.is_null());

        self.len -= 1;
        let ListC(x,next) = unsafe{ self.first.read() };

        if self.len == 0 {
            debug_assert!(next.is_null());
            // list is now empty
            self.first = ptr::null_mut();
            self.last = ptr::null_mut();
        } else {
            // remove first element
            self.first = next;
        }
        x
    }

    /// Append `other` into `self`.
    ///
    /// This modifies `self`, but also possibly `other`.
    fn append(&mut self, other: &mut List<T>) {
        if other.first.is_null() {
            // nothing to do
            debug_assert_eq!(other.len, 0);
        } else if self.first.is_null() {
            // copy other into self
            debug_assert_eq!(self.len, 0);
            self.first = other.first;
            self.last = other.last;
            self.len = other.len;
        } else {
            // make `self.last` point to `other.first`;
            // make `other.first` point to the former `self.last`
            self.len += other.len;

            let n = unsafe{ &mut * self.last };
            debug_assert!(n.1.is_null());
            n.1 = other.first;

            other.first = self.last; // useful when undoing.
            self.last = other.last;
        }
    }

    /// Assuming `self.append(other)` was done earlier, this reverts the operation.
    fn un_append(&mut self, other: &mut List<T>) {
        if other.len == 0 {
            return;
        } else if self.len == other.len {
            // self used to be empty
            self.first = ptr::null_mut();
            self.last = ptr::null_mut();
            self.len = 0;
        } else {
            // `other.first` points to the old `self.last`, so swap them back
            debug_assert!(self.len > other.len);
            self.len -= other.len;

            let last = other.first;
            self.last = last;
            let n = unsafe{ &mut * last };
            other.first = n.1;
            n.1 = ptr::null_mut();
        }
    }
}

impl<AST:Eq+Hash+Debug> Signature<AST> {
    /// Create a new signature.
    fn new() -> Self { Signature(SVec::new()) }

    /// Clear the signature's content.
    #[inline(always)]
    fn clear(&mut self) { self.0.clear() }
}

impl Signature<ast_u32::AST> {
    /// Compute the signature of `f(args)`.
    fn compute_app<C>(
        &mut self, cc1: &mut CC1<C>, f: &C::AST, args: &[C::AST]
    ) where C: Ctx {
        self.clear();
        self.0.push(cc1.find(*f));
        for &u in args {
            self.0.push(cc1.find(u));
        }
    }
}
