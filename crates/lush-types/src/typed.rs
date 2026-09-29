//! Typed-AST handoff for compile step 3 (§15.3).
//!
//! After desugaring, [`crate::node_ids::assign_node_ids`] writes dense ids into
//! every expression and pattern. Inference records side tables keyed by those
//! AST ids; lowering looks them up with [`TypedModule::expr`] /
//! [`TypedModule::pattern`].

use std::collections::{BTreeMap, BTreeSet};

use lush_syntax::ast::Module;
use lush_syntax::span::Span;

use crate::const_eval::ConstValue;
use crate::ty::{Type, TypeDefId, TypeStore};

pub use lush_syntax::ast::NodeId;

/// Unique local binding identity within a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BindingId(pub u32);

/// Where an identifier use resolves.
#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedRef {
    Local(BindingId),
    Function {
        module: String,
        name: String,
    },
    Const {
        module: String,
        name: String,
    },
    Constructor {
        module: String,
        type_def: TypeDefId,
        /// Declaration index among variants of the ADT.
        tag: u16,
        arity: u8,
        labels: Vec<Option<String>>,
    },
    /// Module alias used as a qualifier prefix (not a value).
    ModuleAlias(String),
}

/// Argument-to-parameter mapping for one call (source order).
#[derive(Clone, Debug, PartialEq)]
pub struct CallInfo {
    /// For each source argument: target parameter index.
    pub arg_to_param: Vec<usize>,
    /// True when the last argument is the implicit `use` callback.
    pub use_callback: bool,
    /// True when this call is in tail position.
    pub tail: bool,
}

/// Field access / record update resolution for one receiver type.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSite {
    pub type_def: TypeDefId,
    /// For each variant of the ADT: field index, or `None` if absent.
    pub indices_by_variant: Vec<Option<u16>>,
    pub field_name: String,
}

/// Resolved bit-array segment kind after static checking.
#[derive(Clone, Debug, PartialEq)]
pub enum BitSegmentKind {
    Int {
        size: u8,
        signed: bool,
        little: bool,
    },
    Utf8,
    Bytes {
        size: Option<u32>,
    },
    Bits {
        size: Option<u32>,
    },
}

/// Per-expression typed facts.
#[derive(Clone, Debug)]
pub struct ExprInfo {
    pub id: NodeId,
    pub ty: Type,
    pub span: Span,
    pub resolve: Option<ResolvedRef>,
    pub call: Option<CallInfo>,
    pub field: Option<FieldSite>,
    pub bit_segments: Option<Vec<BitSegmentKind>>,
    /// True when this expression is in tail position (for calls, blocks, etc.).
    pub tail: bool,
}

/// Per-pattern typed facts.
#[derive(Clone, Debug)]
pub struct PatternInfo {
    pub id: NodeId,
    pub ty: Type,
    pub span: Span,
    pub resolve: Option<ResolvedRef>,
    pub bit_segments: Option<Vec<BitSegmentKind>>,
}

/// Fully typed module ready for Core IR lowering.
pub struct TypedModule {
    pub path: String,
    /// Desugared syntactic module (node ids are dense keys into the tables).
    pub module: Module,
    pub store: TypeStore,
    /// Expression facts keyed by the AST node's own [`NodeId`].
    pub exprs: BTreeMap<NodeId, ExprInfo>,
    /// Pattern facts keyed by the AST node's own [`NodeId`].
    pub patterns: BTreeMap<NodeId, PatternInfo>,
    pub const_values: BTreeMap<String, ConstValue>,
    /// Compiler-generated capture / use names; never shown as user names in traces.
    pub gensyms: BTreeSet<String>,
    /// Local binding id → name (deterministic iteration order).
    pub bindings: BTreeMap<BindingId, String>,
}

impl TypedModule {
    pub fn expr(&self, id: NodeId) -> Option<&ExprInfo> {
        self.exprs.get(&id)
    }

    pub fn pattern(&self, id: NodeId) -> Option<&PatternInfo> {
        self.patterns.get(&id)
    }

    /// True when every stored type is fully zonked (no unresolved vars).
    pub fn is_zonked(&self) -> bool {
        self.check_zonked().is_ok()
    }

    /// Ok when every stored type is fully zonked; otherwise an error describing
    /// the first unzonked site.
    pub fn check_zonked(&self) -> Result<(), String> {
        for e in self.exprs.values() {
            if !e.ty.is_zonked() {
                return Err(format!(
                    "unzonked expr type at {:?} (id={:?}): {:?}",
                    e.span, e.id, e.ty
                ));
            }
        }
        for p in self.patterns.values() {
            if !p.ty.is_zonked() {
                return Err(format!(
                    "unzonked pattern type at {:?} (id={:?}): {:?}",
                    p.span, p.id, p.ty
                ));
            }
        }
        Ok(())
    }
}

/// Builder filled during inference.
#[derive(Debug, Default)]
pub struct TypedBuilder {
    pub exprs: BTreeMap<NodeId, ExprInfo>,
    pub patterns: BTreeMap<NodeId, PatternInfo>,
    pub const_values: BTreeMap<String, ConstValue>,
    pub gensyms: BTreeSet<String>,
    pub bindings: BTreeMap<BindingId, String>,
    next_binding: u32,
    /// Stack of tail-position flags for the expression being inferred.
    pub tail_stack: Vec<bool>,
}

impl TypedBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn in_tail(&self) -> bool {
        self.tail_stack.last().copied().unwrap_or(false)
    }

    pub fn push_tail(&mut self, tail: bool) {
        self.tail_stack.push(tail);
    }

    pub fn pop_tail(&mut self) {
        self.tail_stack.pop();
    }

    pub fn alloc_binding(&mut self, name: String) -> BindingId {
        let id = BindingId(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.bindings.insert(id, name);
        id
    }

    /// Record (or refresh) an expression slot using the AST node's own id.
    pub fn begin_expr(&mut self, id: NodeId, span: Span) -> NodeId {
        debug_assert!(!id.is_none(), "begin_expr called with NodeId::NONE");
        self.exprs.insert(
            id,
            ExprInfo {
                id,
                ty: Type::Error,
                span,
                resolve: None,
                call: None,
                field: None,
                bit_segments: None,
                tail: self.in_tail(),
            },
        );
        id
    }

    pub fn finish_expr(&mut self, id: NodeId, ty: Type) {
        if let Some(info) = self.exprs.get_mut(&id) {
            info.ty = ty;
        }
    }

    pub fn set_resolve(&mut self, id: NodeId, r: ResolvedRef) {
        if let Some(info) = self.exprs.get_mut(&id) {
            info.resolve = Some(r);
        }
    }

    pub fn set_call(&mut self, id: NodeId, call: CallInfo) {
        if let Some(info) = self.exprs.get_mut(&id) {
            info.call = Some(call);
        }
    }

    pub fn set_field(&mut self, id: NodeId, field: FieldSite) {
        if let Some(info) = self.exprs.get_mut(&id) {
            info.field = Some(field);
        }
    }

    pub fn set_bit_segments(&mut self, id: NodeId, segs: Vec<BitSegmentKind>) {
        if let Some(info) = self.exprs.get_mut(&id) {
            info.bit_segments = Some(segs);
        }
    }

    pub fn begin_pattern(&mut self, id: NodeId, span: Span) -> NodeId {
        debug_assert!(!id.is_none(), "begin_pattern called with NodeId::NONE");
        self.patterns.insert(
            id,
            PatternInfo {
                id,
                ty: Type::Error,
                span,
                resolve: None,
                bit_segments: None,
            },
        );
        id
    }

    pub fn finish_pattern(&mut self, id: NodeId, ty: Type) {
        if let Some(info) = self.patterns.get_mut(&id) {
            info.ty = ty;
        }
    }

    pub fn set_pattern_resolve(&mut self, id: NodeId, r: ResolvedRef) {
        if let Some(info) = self.patterns.get_mut(&id) {
            info.resolve = Some(r);
        }
    }

    pub fn set_pattern_bit_segments(&mut self, id: NodeId, segs: Vec<BitSegmentKind>) {
        if let Some(info) = self.patterns.get_mut(&id) {
            info.bit_segments = Some(segs);
        }
    }

    pub fn build(self, path: String, module: Module, store: TypeStore) -> TypedModule {
        TypedModule {
            path,
            module,
            store,
            exprs: self.exprs,
            patterns: self.patterns,
            const_values: self.const_values,
            gensyms: self.gensyms,
            bindings: self.bindings,
        }
    }
}
