//! Deterministic typed-AST indexing and annotation slots.
//!
//! This module indexes the desugared syntax tree without changing it. Inference
//! can use [`NodeIndex`] to associate work performed against `CheckResult.module`
//! with stable, module-local annotation IDs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use lush_syntax::ast::{
    self, ArgValue, BinOp, Block, Expr, ExprKind, Module, ModuleItem, Pattern, PatternKind,
    Statement,
};

use crate::const_eval::ConstValue;
use crate::ty::{ConstraintSet, RigidId, TvId, Type, TypeStore};
use lush_syntax::span::Span;

/// A deterministic, module-local identifier for an expression or pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Identity of one lexical binding in a checked module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BindingId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct BindingAnnotation {
    pub id: BindingId,
    pub name: String,
    pub span: Span,
    pub local: bool,
    pub compiler_generated: bool,
}

impl BindingAnnotation {
    /// Name suitable for user-facing traces; generated temporaries have no user name.
    pub fn user_visible_name(&self) -> Option<&str> {
        (!self.compiler_generated).then_some(self.name.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedValueKind {
    Function,
    Constant,
    Constructor {
        type_def: Option<crate::ty::TypeDefId>,
        variant_tag: Option<u32>,
        arity: usize,
        field_labels: Vec<Option<String>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedReference {
    LocalBinding(BindingId),
    TopLevel {
        module: String,
        name: String,
        kind: ResolvedValueKind,
    },
    Imported {
        module: String,
        name: String,
        kind: ResolvedValueKind,
    },
    ModuleAlias {
        module: String,
    },
}

/// Inference annotations for one expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitSegmentKind {
    Integer,
    Utf8,
    Bytes,
    Bits,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitSegmentAnnotation {
    pub kind: BitSegmentKind,
    /// Literal width/length when statically known; integer segments default to 8.
    pub size: Option<i64>,
    /// Node id of a non-literal size expression, if present.
    pub size_expr: Option<NodeId>,
    /// `Some(false)` is the integer-segment default (unsigned).
    pub signed: Option<bool>,
    /// False is the default big-endian order.
    pub little_endian: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantFieldPosition {
    pub variant_tag: u32,
    pub field_index: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprAnnotation {
    pub id: NodeId,
    pub span: Span,
    /// Filled by inference; indexing deliberately leaves this empty.
    pub ty: Option<Type>,
    /// Whether this expression occupies a structural tail position (§5.2).
    pub tail_position: bool,
    /// This node is a module namespace qualifier, not a value expression.
    pub module_qualifier: bool,
    /// Parameter index for each source-order argument of this call.
    pub argument_to_parameter: Option<Vec<Option<usize>>>,
    /// Source-order argument index for a desugared `use` callback, if any.
    pub implicit_use_callback_argument: Option<usize>,
    pub resolved_reference: Option<ResolvedReference>,
    /// Physical field position for each variant supporting this field access.
    pub field_positions: Option<Vec<VariantFieldPosition>>,
    /// Receiver type used to resolve field access or a record update.
    pub receiver_ty: Option<Type>,
    /// Resolved options for each source-order bit-array expression segment.
    pub bit_segments: Option<Vec<BitSegmentAnnotation>>,
}

/// Inference annotations for one pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternAnnotation {
    pub id: NodeId,
    pub span: Span,
    pub resolved_reference: Option<ResolvedReference>,
    /// Filled by inference; indexing deliberately leaves this empty.
    pub ty: Option<Type>,
    /// Resolved options for each source-order bit-array pattern segment.
    pub bit_segments: Option<Vec<BitSegmentAnnotation>>,
}

/// The annotation slots and private source-node lookup for a module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypedModule {
    pub expressions: BTreeMap<NodeId, ExprAnnotation>,
    pub patterns: BTreeMap<NodeId, PatternAnnotation>,
    pub bindings: BTreeMap<BindingId, BindingAnnotation>,
    /// Evaluated constants keyed by their declaration name.
    pub constants: BTreeMap<String, ConstValue>,
    /// Names introduced by capture desugaring; never display these as user names.
    pub generated_names: std::collections::BTreeSet<String>,
    /// Constraints attached to quantified variables represented as `Type::Rigid`.
    pub rigid_constraints: BTreeMap<RigidId, ConstraintSet>,
    pub(crate) index: NodeIndex,
}

impl TypedModule {
    /// Return the annotation ID for a node in the exact desugared module paired
    /// with this result. IDs are not persistent across source edits.
    pub fn expr_id(&self, expr: &Expr) -> Option<NodeId> {
        self.index.expr_id(expr)
    }

    /// Return the annotation ID for a pattern in the paired desugared module.
    pub fn pattern_id(&self, pattern: &Pattern) -> Option<NodeId> {
        self.index.pattern_id(pattern)
    }

    pub fn expression(&self, id: NodeId) -> Option<&ExprAnnotation> {
        self.expressions.get(&id)
    }

    pub fn pattern(&self, id: NodeId) -> Option<&PatternAnnotation> {
        self.patterns.get(&id)
    }

    /// Count distinct compound constant nodes, following shared substructure once.
    pub fn constant_node_count(&self) -> usize {
        let mut seen = HashSet::new();
        let mut pending: Vec<&ConstValue> = self.constants.values().collect();
        while let Some(value) = pending.pop() {
            match value {
                ConstValue::Tuple(items) | ConstValue::List(items) => {
                    if seen.insert(Rc::as_ptr(items) as usize) {
                        pending.extend(items.iter());
                    }
                }
                ConstValue::Adt { fields, .. } => {
                    if seen.insert(Rc::as_ptr(fields) as usize) {
                        pending.extend(fields.iter());
                    }
                }
                ConstValue::Int(_)
                | ConstValue::Float(_)
                | ConstValue::String(_)
                | ConstValue::Bool(_)
                | ConstValue::Nil => {}
            }
        }
        seen.len()
    }

    pub(crate) fn record_binding(&mut self, binding: BindingAnnotation) {
        self.bindings.insert(binding.id, binding);
    }

    pub(crate) fn record_reference(&mut self, expr: &Expr, reference: ResolvedReference) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.resolved_reference = Some(reference);
            }
        }
    }

    pub(crate) fn record_pattern_reference(
        &mut self,
        pattern: &Pattern,
        reference: ResolvedReference,
    ) {
        if let Some(id) = self.index.pattern_id(pattern) {
            if let Some(annotation) = self.patterns.get_mut(&id) {
                annotation.resolved_reference = Some(reference);
            }
        }
    }

    pub(crate) fn record_call_mapping(&mut self, expr: &Expr, mapping: Vec<Option<usize>>) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.argument_to_parameter = Some(mapping);
            }
        }
    }

    pub(crate) fn record_expr_bit_segments(
        &mut self,
        expr: &Expr,
        segments: Vec<BitSegmentAnnotation>,
    ) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.bit_segments = Some(segments);
            }
        }
    }

    pub(crate) fn record_pattern_bit_segments(
        &mut self,
        pattern: &Pattern,
        segments: Vec<BitSegmentAnnotation>,
    ) {
        if let Some(id) = self.index.pattern_id(pattern) {
            if let Some(annotation) = self.patterns.get_mut(&id) {
                annotation.bit_segments = Some(segments);
            }
        }
    }

    pub(crate) fn record_receiver_type(&mut self, expr: &Expr, ty: Type) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.receiver_ty = Some(ty);
            }
        }
    }

    pub(crate) fn record_field_positions(
        &mut self,
        expr: &Expr,
        positions: Vec<VariantFieldPosition>,
    ) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.field_positions = Some(positions);
            }
        }
    }

    pub(crate) fn record_module_qualifier(&mut self, expr: &Expr) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.module_qualifier = true;
            }
        }
    }

    pub(crate) fn record_expr(&mut self, expr: &Expr, ty: Type) {
        if let Some(id) = self.index.expr_id(expr) {
            if let Some(annotation) = self.expressions.get_mut(&id) {
                annotation.ty = Some(ty);
            }
        }
    }

    pub(crate) fn record_pattern(&mut self, pattern: &Pattern, ty: Type) {
        if let Some(id) = self.index.pattern_id(pattern) {
            if let Some(annotation) = self.patterns.get_mut(&id) {
                annotation.ty = Some(ty);
            }
        }
    }

    pub(crate) fn zonk_types(&mut self, store: &mut TypeStore) {
        let mut vars = BTreeMap::new();
        let mut zonk_memo = HashMap::new();
        let mut freeze_memo = HashMap::new();
        for annotation in self.expressions.values_mut() {
            if let Some(ty) = &annotation.ty {
                let root = Rc::new(ty.clone());
                let zonked = store.zonk_rc(&root, &mut zonk_memo);
                annotation.ty =
                    Some((*freeze_type_rc(&zonked, &mut vars, &mut freeze_memo)).clone());
            }
            if let Some(ty) = &annotation.receiver_ty {
                let root = Rc::new(ty.clone());
                let zonked = store.zonk_rc(&root, &mut zonk_memo);
                annotation.receiver_ty =
                    Some((*freeze_type_rc(&zonked, &mut vars, &mut freeze_memo)).clone());
            }
        }
        for annotation in self.patterns.values_mut() {
            if let Some(ty) = &annotation.ty {
                let root = Rc::new(ty.clone());
                let zonked = store.zonk_rc(&root, &mut zonk_memo);
                annotation.ty =
                    Some((*freeze_type_rc(&zonked, &mut vars, &mut freeze_memo)).clone());
            }
        }
        self.rigid_constraints = vars
            .into_iter()
            .map(|(tv, ordinal)| {
                let rigid = RigidId((1_u64 << 63) | u64::from(ordinal));
                let constraints = store
                    .vars
                    .get(&tv)
                    .map(|info| info.constraints.clone())
                    .unwrap_or_default();
                (rigid, constraints)
            })
            .collect();
    }
}

fn freeze_type_rc(
    ty: &Rc<Type>,
    vars: &mut BTreeMap<TvId, u32>,
    memo: &mut HashMap<*const Type, Rc<Type>>,
) -> Rc<Type> {
    let key = Rc::as_ptr(ty);
    if let Some(frozen) = memo.get(&key) {
        return frozen.clone();
    }
    let frozen = match ty.as_ref() {
        Type::Var(id) => {
            let next = vars.len().min(u32::MAX as usize) as u32;
            let ordinal = *vars.entry(*id).or_insert(next);
            Rc::new(Type::Rigid(RigidId((1_u64 << 63) | u64::from(ordinal))))
        }
        Type::List(inner) => Rc::new(Type::List(freeze_type_rc(inner, vars, memo))),
        Type::Tuple(items) => Rc::new(Type::Tuple(
            items
                .iter()
                .map(|item| freeze_type_rc(item, vars, memo))
                .collect(),
        )),
        Type::Fun { params, ret } => Rc::new(Type::Fun {
            params: params
                .iter()
                .map(|param| freeze_type_rc(param, vars, memo))
                .collect(),
            ret: freeze_type_rc(ret, vars, memo),
        }),
        Type::App { def, args } => Rc::new(Type::App {
            def: *def,
            args: args
                .iter()
                .map(|arg| freeze_type_rc(arg, vars, memo))
                .collect(),
        }),
        Type::Error
        | Type::Rigid(_)
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Bool
        | Type::Nil
        | Type::BitArray => Rc::new(ty.as_ref().clone()),
    };
    memo.insert(key, frozen.clone());
    frozen
}

/// Pointer-based lookup built while walking the same, unchanged AST.
///
/// Pointer addresses are only keys for this indexing pass; IDs themselves are
/// assigned by traversal order and do not depend on addresses or hash iteration.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NodeIndex {
    expr_ids: BTreeMap<usize, NodeId>,
    pattern_ids: BTreeMap<usize, NodeId>,
}

impl NodeIndex {
    pub(crate) fn expr_id(&self, expr: &Expr) -> Option<NodeId> {
        self.expr_ids.get(&(expr as *const Expr as usize)).copied()
    }

    pub(crate) fn pattern_id(&self, pattern: &Pattern) -> Option<NodeId> {
        self.pattern_ids
            .get(&(pattern as *const Pattern as usize))
            .copied()
    }
}

/// Assign IDs to every expression and pattern in a desugared module.
///
/// IDs are contiguous from zero in deterministic source-order depth-first
/// traversal, numbering expressions and patterns in one shared sequence.
pub fn index_module(module: &Module) -> TypedModule {
    let mut indexer = Indexer::default();
    for item in &module.items {
        match item {
            ModuleItem::Fn(function) => indexer.block(&function.body, true),
            ModuleItem::Const(constant) => indexer.expr(&constant.value, false),
            ModuleItem::Import(_) | ModuleItem::Type(_) => {}
        }
    }
    TypedModule {
        expressions: indexer.expressions,
        patterns: indexer.patterns,
        bindings: BTreeMap::new(),
        constants: BTreeMap::new(),
        generated_names: std::collections::BTreeSet::new(),
        rigid_constraints: BTreeMap::new(),
        index: indexer.index,
    }
}

#[derive(Default)]
struct Indexer {
    next_id: u32,
    expressions: BTreeMap<NodeId, ExprAnnotation>,
    patterns: BTreeMap<NodeId, PatternAnnotation>,
    index: NodeIndex,
}

impl Indexer {
    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn expr(&mut self, expr: &Expr, tail_position: bool) {
        let id = self.alloc_id();
        self.index.expr_ids.insert(expr as *const Expr as usize, id);
        let implicit_use_callback_argument = match &expr.kind {
            ExprKind::Call { args, .. } => args.iter().position(|arg| arg.implicit_use_callback),
            _ => None,
        };
        self.expressions.insert(
            id,
            ExprAnnotation {
                id,
                span: expr.span,
                ty: None,
                tail_position,
                module_qualifier: false,
                argument_to_parameter: None,
                implicit_use_callback_argument,
                resolved_reference: None,
                field_positions: None,
                receiver_ty: None,
                bit_segments: None,
            },
        );

        match &expr.kind {
            ExprKind::Tuple(items) => {
                for item in items {
                    self.expr(item, false);
                }
            }
            ExprKind::List { items, spread } => {
                for item in items {
                    self.expr(item, false);
                }
                if let Some(spread) = spread {
                    self.expr(spread, false);
                }
            }
            ExprKind::BitArray(segments) => {
                for segment in segments {
                    self.expr(&segment.value, false);
                    self.bit_options(&segment.options);
                }
            }
            ExprKind::RecordUpdate { base, fields, .. } => {
                self.expr(base, false);
                for (_, value) in fields {
                    self.expr(value, false);
                }
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee, false);
                for arg in args {
                    if let ArgValue::Expr(value) = &arg.value {
                        self.expr(value, false);
                    }
                }
            }
            ExprKind::Field { base, .. } => self.expr(base, false),
            ExprKind::Binary { left, op, right } => {
                self.expr(left, false);
                self.expr(right, tail_position && matches!(op, BinOp::And | BinOp::Or));
            }
            ExprKind::Unary { expr, .. } | ExprKind::Assert { expr, .. } | ExprKind::Echo(expr) => {
                self.expr(expr, false)
            }
            ExprKind::Pipe { left, right } => {
                self.expr(left, false);
                self.expr(right, false);
            }
            ExprKind::Fn { body, .. } => self.block(body, true),
            ExprKind::Case { subjects, clauses } => {
                for subject in subjects {
                    self.expr(subject, false);
                }
                for clause in clauses {
                    for row in &clause.patterns {
                        for pattern in &row.patterns {
                            self.pattern(pattern);
                        }
                    }
                    if let Some(guard) = &clause.guard {
                        self.expr(guard, false);
                    }
                    self.expr(&clause.body, tail_position);
                }
            }
            ExprKind::Block(block) => self.block(block, tail_position),
            ExprKind::Paren(inner) => self.expr(inner, tail_position),
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Var(_)
            | ExprKind::Constructor(_)
            | ExprKind::Todo { .. }
            | ExprKind::Panic { .. } => {}
        }
    }

    fn block(&mut self, block: &Block, tail_block: bool) {
        let final_index = block.statements.len().checked_sub(1);
        for (position, statement) in block.statements.iter().enumerate() {
            let is_tail_expr = tail_block
                && Some(position) == final_index
                && matches!(statement, Statement::Expr(_));
            match statement {
                Statement::Fn(function) => self.block(&function.body, true),
                Statement::Let(let_stmt) => {
                    self.pattern(&let_stmt.pattern);
                    self.expr(&let_stmt.value, false);
                }
                Statement::Use(use_stmt) => {
                    for pattern in &use_stmt.patterns {
                        self.pattern(pattern);
                    }
                    self.expr(&use_stmt.value, false);
                }
                Statement::Expr(expr) => self.expr(expr, is_tail_expr),
            }
        }
    }

    fn pattern(&mut self, pattern: &Pattern) {
        let id = self.alloc_id();
        self.index
            .pattern_ids
            .insert(pattern as *const Pattern as usize, id);
        self.patterns.insert(
            id,
            PatternAnnotation {
                id,
                span: pattern.span,
                resolved_reference: None,
                ty: None,
                bit_segments: None,
            },
        );

        match &pattern.kind {
            PatternKind::Constructor {
                args: Some(args), ..
            } => {
                for arg in args {
                    if let Some(pattern) = &arg.pattern {
                        self.pattern(pattern);
                    }
                }
            }
            PatternKind::Tuple(items) => {
                for item in items {
                    self.pattern(item);
                }
            }
            PatternKind::List { items, spread } => {
                for item in items {
                    self.pattern(item);
                }
                if let Some(spread) = spread {
                    self.pattern(spread);
                }
            }
            PatternKind::BitArray(segments) => {
                for segment in segments {
                    self.pattern(&segment.pattern);
                    self.bit_options(&segment.options);
                }
            }
            PatternKind::StringPrefix { rest, .. } => self.pattern(rest),
            PatternKind::Alias { pattern, .. } => self.pattern(pattern),
            PatternKind::Int(_)
            | PatternKind::Float(_)
            | PatternKind::String(_)
            | PatternKind::Var(_)
            | PatternKind::Discard
            | PatternKind::UnderscoreName(_)
            | PatternKind::Constructor { args: None, .. } => {}
        }
    }

    fn bit_options(&mut self, options: &[ast::BitOption]) {
        for option in options {
            if let ast::BitOption::Size(size) = option {
                self.expr(size, false);
            }
        }
    }
}
