
//! AST manager
//!
//! The AST manager stores AST nodes, referred to via `ID`. These nodes can
//! be used to represent sorts, terms, formulas, theory terms, etc.

use smallvec::SmallVec;
use super::symbol::{Symbol,SymbolManager};
use fxhash::FxHashMap;
use std::collections::hash_map::Entry;

/// The unique identifier of an AST node.
#[derive(Copy,Clone,Eq,PartialEq,Hash,Ord,PartialOrd,Debug)]
pub struct AST(u32);

/// Number of arguments for a "small" term application
pub const N_SMALL_APP : usize = 2;

/// The definition of an AST node
#[derive(Debug,Eq,PartialEq,Ord,PartialOrd,Hash,Clone)]
pub enum Node {
    Const(Symbol),
    App {
        f: AST,
        args: SmallVec<[AST; N_SMALL_APP]>,
    }
}


pub struct AstManager {
    nodes: Vec<Node>,
    tbl: FxHashMap<Node, AST>, // for hashconsing
    syms: SymbolManager,
}

impl AstManager {
    /// Create a new AST manager
    pub fn new() -> Self {
        AstManager::with_symbol_manager(SymbolManager::new())
    }

    pub fn with_symbol_manager(m: SymbolManager) -> Self {
        // TODO: add "kind" as first entry, with itself as type?
        AstManager {
            nodes: Vec::with_capacity(1_024),
            tbl: FxHashMap::default(),
            syms: m,
        }
    }

    #[inline(always)]
    pub fn syms(&self) -> &SymbolManager { &self.syms }

    #[inline(always)]
    pub fn syms_mut(&mut self) -> &mut SymbolManager { &mut self.syms }

    /// Number of terms
    pub fn len(&self) -> usize { self.nodes.len() }

    // Add or retrieve the unique term with this definition
    fn hashcons(&mut self, node: Node) -> AST {
        // borrow multiple fields
        let AstManager {ref mut nodes, ref mut tbl,..} = self;
        match tbl.entry(node) {
            Entry::Vacant(ve) => {
                // insert
                let n = nodes.len();
                let node2 = ve.key().clone(); // copy the key
                nodes.push(node2);
                let ast = AST(n as u32);
                ve.insert(ast);
                ast
            },
            Entry::Occupied(e) => *e.get(),
        }
    }

    pub fn mk_const(&mut self, s: Symbol) -> AST {
        let node = Node::Const(s);
        self.hashcons(node)
    }

    pub fn mk_app(&mut self, f: AST, args: &[AST]) -> AST {
        // TODO: if already present, we should not need to copy :s
        let node = Node::App{f, args: SmallVec::from_slice(args)};
        self.hashcons(node)
    }
}
