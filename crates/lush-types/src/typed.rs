//! Typed-AST handoff for compile step 3 (§15.3).
//!
//! Node identities are preorder indices assigned while walking the desugared
//! module. Inference records side tables in the same visit order; lowering
//! consumes them through [`TypedCursor`].

use std::collections::{HashMap, HashSet};

use lush_syntax::ast::Module;
use lush_syntax::span::Span;

use crate::const_eval::ConstValue;
use crate::ty::{Type, TypeDefId, TypeStore};

pub use lush_syntax::ast::NodeId;

/// Unique local binding identity within a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Desugared syntactic module (node ids are preorder indices into the tables).
    pub module: Module,
    pub store: TypeStore,
    pub exprs: Vec<ExprInfo>,
    pub patterns: Vec<PatternInfo>,
    pub const_values: HashMap<String, ConstValue>,
    /// Compiler-generated capture / use names; never shown as user names in traces.
    pub gensyms: HashSet<String>,
    /// Local binding names → binding id (for the module's binding table).
    pub bindings: HashMap<BindingId, String>,
}

impl TypedModule {
    pub fn expr(&self, id: NodeId) -> &ExprInfo {
        &self.exprs[id.0 as usize]
    }

    pub fn pattern(&self, id: NodeId) -> &PatternInfo {
        &self.patterns[id.0 as usize]
    }

    /// Assert every stored type is fully zonked (no unresolved vars).
    pub fn assert_zonked(&self) {
        for e in &self.exprs {
            assert!(
                e.ty.is_zonked(),
                "unzonked expr type at {:?}: {:?}",
                e.span,
                e.ty
            );
        }
        for p in &self.patterns {
            assert!(
                p.ty.is_zonked(),
                "unzonked pattern type at {:?}: {:?}",
                p.span,
                p.ty
            );
        }
    }
}

/// Builder filled during inference.
#[derive(Debug, Default)]
pub struct TypedBuilder {
    pub exprs: Vec<ExprInfo>,
    pub patterns: Vec<PatternInfo>,
    pub const_values: HashMap<String, ConstValue>,
    pub gensyms: HashSet<String>,
    pub bindings: HashMap<BindingId, String>,
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

    pub fn begin_expr(&mut self, span: Span) -> NodeId {
        let id = NodeId(self.exprs.len() as u32);
        self.exprs.push(ExprInfo {
            id,
            ty: Type::Error,
            span,
            resolve: None,
            call: None,
            field: None,
            bit_segments: None,
            tail: self.in_tail(),
        });
        id
    }

    pub fn finish_expr(&mut self, id: NodeId, ty: Type) {
        if let Some(info) = self.exprs.get_mut(id.0 as usize) {
            info.ty = ty;
        }
    }

    pub fn set_resolve(&mut self, id: NodeId, r: ResolvedRef) {
        if let Some(info) = self.exprs.get_mut(id.0 as usize) {
            info.resolve = Some(r);
        }
    }

    pub fn set_call(&mut self, id: NodeId, call: CallInfo) {
        if let Some(info) = self.exprs.get_mut(id.0 as usize) {
            info.call = Some(call);
        }
    }

    pub fn set_field(&mut self, id: NodeId, field: FieldSite) {
        if let Some(info) = self.exprs.get_mut(id.0 as usize) {
            info.field = Some(field);
        }
    }

    pub fn set_bit_segments(&mut self, id: NodeId, segs: Vec<BitSegmentKind>) {
        if let Some(info) = self.exprs.get_mut(id.0 as usize) {
            info.bit_segments = Some(segs);
        }
    }

    pub fn begin_pattern(&mut self, span: Span) -> NodeId {
        let id = NodeId(self.patterns.len() as u32);
        self.patterns.push(PatternInfo {
            id,
            ty: Type::Error,
            span,
            resolve: None,
            bit_segments: None,
        });
        id
    }

    pub fn finish_pattern(&mut self, id: NodeId, ty: Type) {
        if let Some(info) = self.patterns.get_mut(id.0 as usize) {
            info.ty = ty;
        }
    }

    pub fn set_pattern_resolve(&mut self, id: NodeId, r: ResolvedRef) {
        if let Some(info) = self.patterns.get_mut(id.0 as usize) {
            info.resolve = Some(r);
        }
    }

    pub fn set_pattern_bit_segments(&mut self, id: NodeId, segs: Vec<BitSegmentKind>) {
        if let Some(info) = self.patterns.get_mut(id.0 as usize) {
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

/// Cursor that yields [`NodeId`]s in the same preorder used by inference.
pub struct TypedCursor<'a> {
    exprs: &'a [ExprInfo],
    patterns: &'a [PatternInfo],
    ei: usize,
    pi: usize,
}

impl<'a> TypedCursor<'a> {
    pub fn new(module: &'a TypedModule) -> Self {
        Self {
            exprs: &module.exprs,
            patterns: &module.patterns,
            ei: 0,
            pi: 0,
        }
    }

    pub fn next_expr(&mut self) -> &'a ExprInfo {
        let info = &self.exprs[self.ei];
        self.ei += 1;
        info
    }

    pub fn next_pattern(&mut self) -> &'a PatternInfo {
        let info = &self.patterns[self.pi];
        self.pi += 1;
        info
    }

    pub fn exprs_remaining(&self) -> usize {
        self.exprs.len().saturating_sub(self.ei)
    }

    pub fn patterns_remaining(&self) -> usize {
        self.patterns.len().saturating_sub(self.pi)
    }
}
