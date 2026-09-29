//! Register bytecode (§8).

use crate::limits::MAX_REGISTERS;

pub type Reg = u8;
pub type FuncId = u32;
pub type ConstId = u32;

#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Int(i64),
    Float(f64),
    String(String),
    Nullary { type_tag: u16, variant: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Println,
    Eprintln,
    IntToString,
}

/// How to encode one segment when building a bit array (`MakeBitArray`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BitSegEnc {
    Int {
        size: u8,
        signed: bool,
        little: bool,
    },
    Utf8,
    /// Append a `BitArray`; `size_bits = None` means the whole value.
    Bits {
        size_bits: Option<u32>,
    },
    /// Append a byte-aligned `BitArray`; `size_bytes = None` means the whole value.
    Bytes {
        size_bytes: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Move {
        dst: Reg,
        src: Reg,
    },
    LoadConst {
        dst: Reg,
        idx: ConstId,
    },
    LoadInt {
        dst: Reg,
        value: i64,
    },
    LoadBool {
        dst: Reg,
        value: bool,
    },
    LoadNil {
        dst: Reg,
    },
    Add {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Sub {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mul {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Div {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Rem {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Neg {
        dst: Reg,
        src: Reg,
    },
    Concat {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Eq {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Ne {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Lt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Le {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Gt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Ge {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Jump {
        target: u32,
    },
    JumpIfFalse {
        cond: Reg,
        target: u32,
    },
    Call {
        dst: Reg,
        func: FuncId,
        args: Vec<Reg>,
    },
    TailCall {
        func: FuncId,
        args: Vec<Reg>,
    },
    CallClosure {
        dst: Reg,
        clo: Reg,
        args: Vec<Reg>,
    },
    TailCallClosure {
        clo: Reg,
        args: Vec<Reg>,
    },
    Return {
        src: Reg,
    },
    Panic {
        msg: ConstId,
    },
    Builtin {
        dst: Reg,
        builtin: Builtin,
        args: Vec<Reg>,
    },
    MakeTuple {
        dst: Reg,
        fields: Vec<Reg>,
    },
    MakeCons {
        dst: Reg,
        head: Reg,
        tail: Reg,
    },
    MakeEmptyList {
        dst: Reg,
    },
    /// `dst = closure(func, captures…)` — captures are taken by value from registers.
    MakeClosure {
        dst: Reg,
        func: FuncId,
        captures: Vec<Reg>,
    },
    MakeAdt {
        dst: Reg,
        type_tag: u16,
        variant: u16,
        fields: Vec<Reg>,
    },
    GetField {
        dst: Reg,
        base: Reg,
        index: u16,
    },
    SwitchTag {
        scrutinee: Reg,
        arms: Vec<(u16, u32)>,
        default: u32,
    },
    IsInt {
        dst: Reg,
        src: Reg,
        value: i64,
    },
    /// `dst = src` is the empty list.
    IsEmptyList {
        dst: Reg,
        src: Reg,
    },
    /// Write location + inspect(src) to stderr; result is src.
    Echo {
        dst: Reg,
        src: Reg,
    },
    /// Build a bit array: `values[i]` encoded per `specs[i]`. Panics on range/size errors.
    MakeBitArray {
        dst: Reg,
        values: Vec<Reg>,
        specs: Vec<BitSegEnc>,
    },
    /// Read an integer segment; on success write `value` and `rest`, set `ok` true.
    /// Insufficient input or an out-of-range unsigned-64 decode sets `ok` false (no panic).
    BitArrayTakeInt {
        ok: Reg,
        value: Reg,
        rest: Reg,
        src: Reg,
        size: u8,
        signed: bool,
        little: bool,
    },
    /// Match a literal UTF-8 prefix (compare against `expected` String); write `rest`.
    BitArrayTakeUtf8 {
        ok: Reg,
        rest: Reg,
        src: Reg,
        expected: Reg,
    },
    /// Bind the remainder as a bit array. When `require_byte_aligned`, fail if not aligned.
    BitArrayTakeRest {
        ok: Reg,
        value: Reg,
        src: Reg,
        require_byte_aligned: bool,
    },
    /// `dst = true` when `src` has zero remaining bits.
    BitArrayIsEmpty {
        dst: Reg,
        src: Reg,
    },
    /// Match a UTF-8 string prefix (`"ab" <> rest`); write `rest` on success.
    StringTakePrefix {
        ok: Reg,
        rest: Reg,
        src: Reg,
        expected: Reg,
    },
}

#[derive(Clone, Debug)]
pub struct Function {
    pub module: String,
    pub name: String,
    pub arity: u8,
    /// Captured values occupy registers `arity .. arity + n_captures` after entry
    /// via `CallClosure`, or after a same-group `Call`/`TailCall` that copies them
    /// from the caller frame.
    pub n_captures: u8,
    /// Number of registers used (indices `0 .. regs`). May be 256 (`MAX_REGISTERS`).
    pub regs: u16,
    pub code: Vec<Op>,
    /// Per-instruction (line, col) — 1-based; (0,0) if unknown.
    pub lines: Vec<(u32, u32)>,
    /// Module-relative source path (`src/<module>.lush`).
    pub source_path: String,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<Function>,
    pub constants: Vec<Constant>,
    pub entry: FuncId,
    /// Module path → source text (for panic line/col).
    pub sources: std::collections::BTreeMap<String, String>,
}

impl Program {
    pub fn verify(&self) -> Result<(), String> {
        if self.entry as usize >= self.functions.len() {
            return Err("entry function index out of range".into());
        }
        for (fi, f) in self.functions.iter().enumerate() {
            if f.regs as usize > MAX_REGISTERS {
                return Err(format!("function {fi} exceeds register limit"));
            }
            if f.code.is_empty() {
                return Err(format!("function {fi} has no instructions"));
            }
            let last = f.code.last().unwrap();
            if !matches!(
                last,
                Op::Return { .. }
                    | Op::TailCall { .. }
                    | Op::TailCallClosure { .. }
                    | Op::Panic { .. }
            ) {
                return Err(format!("function {fi} missing terminator"));
            }
            for (pi, op) in f.code.iter().enumerate() {
                verify_op(self, f, fi, pi, op)?;
            }
        }
        Ok(())
    }
}

fn reg_ok(f: &Function, r: Reg) -> Result<(), String> {
    if (r as usize) < f.regs as usize {
        Ok(())
    } else {
        Err(format!("register {r} out of range (regs={})", f.regs))
    }
}

fn verify_op(prog: &Program, f: &Function, fi: usize, pi: usize, op: &Op) -> Result<(), String> {
    let jump_ok = |t: u32| -> Result<(), String> {
        if (t as usize) < f.code.len() {
            // <= allows landing pads one past last (filled by a later Return)
            Ok(())
        } else {
            Err(format!(
                "jump target {t} out of range in function {fi}:{pi}"
            ))
        }
    };
    let const_ok = |c: ConstId| -> Result<(), String> {
        if (c as usize) < prog.constants.len() {
            Ok(())
        } else {
            Err(format!("const {c} out of range in function {fi}:{pi}"))
        }
    };
    let func_ok = |g: FuncId| -> Result<(), String> {
        if (g as usize) < prog.functions.len() {
            Ok(())
        } else {
            Err(format!("func {g} out of range in function {fi}:{pi}"))
        }
    };
    match op {
        Op::Move { dst, src } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *src)?;
        }
        Op::LoadConst { dst, idx } => {
            reg_ok(f, *dst)?;
            const_ok(*idx)?;
        }
        Op::LoadInt { dst, .. } | Op::LoadBool { dst, .. } | Op::LoadNil { dst } => {
            reg_ok(f, *dst)?;
        }
        Op::Add { dst, a, b }
        | Op::Sub { dst, a, b }
        | Op::Mul { dst, a, b }
        | Op::Div { dst, a, b }
        | Op::Rem { dst, a, b }
        | Op::Concat { dst, a, b }
        | Op::Eq { dst, a, b }
        | Op::Ne { dst, a, b }
        | Op::Lt { dst, a, b }
        | Op::Le { dst, a, b }
        | Op::Gt { dst, a, b }
        | Op::Ge { dst, a, b } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *a)?;
            reg_ok(f, *b)?;
        }
        Op::Neg { dst, src }
        | Op::IsInt { dst, src, .. }
        | Op::IsEmptyList { dst, src }
        | Op::Echo { dst, src } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *src)?;
        }
        Op::Jump { target } => jump_ok(*target)?,
        Op::JumpIfFalse { cond, target } => {
            reg_ok(f, *cond)?;
            jump_ok(*target)?;
        }
        Op::Call { dst, func, args } => {
            reg_ok(f, *dst)?;
            func_ok(*func)?;
            for a in args {
                reg_ok(f, *a)?;
            }
            let arity = prog.functions[*func as usize].arity as usize;
            if args.len() != arity {
                return Err(format!("arity mismatch on call in {fi}:{pi}"));
            }
        }
        Op::TailCall { func, args } => {
            func_ok(*func)?;
            for a in args {
                reg_ok(f, *a)?;
            }
            let arity = prog.functions[*func as usize].arity as usize;
            if args.len() != arity {
                return Err(format!("arity mismatch on tail call in {fi}:{pi}"));
            }
        }
        Op::CallClosure { dst, clo, args } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *clo)?;
            for a in args {
                reg_ok(f, *a)?;
            }
        }
        Op::TailCallClosure { clo, args } => {
            reg_ok(f, *clo)?;
            for a in args {
                reg_ok(f, *a)?;
            }
        }
        Op::Return { src } => reg_ok(f, *src)?,
        Op::Panic { msg } => const_ok(*msg)?,
        Op::Builtin { dst, args, .. } => {
            reg_ok(f, *dst)?;
            for a in args {
                reg_ok(f, *a)?;
            }
        }
        Op::MakeTuple { dst, fields } | Op::MakeAdt { dst, fields, .. } => {
            reg_ok(f, *dst)?;
            for a in fields {
                reg_ok(f, *a)?;
            }
        }
        Op::MakeCons { dst, head, tail } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *head)?;
            reg_ok(f, *tail)?;
        }
        Op::MakeEmptyList { dst } => reg_ok(f, *dst)?,
        Op::MakeClosure {
            dst,
            func,
            captures,
        } => {
            reg_ok(f, *dst)?;
            func_ok(*func)?;
            for a in captures {
                reg_ok(f, *a)?;
            }
            let n_caps = prog.functions[*func as usize].n_captures as usize;
            if captures.len() != n_caps {
                return Err(format!(
                    "capture count mismatch on MakeClosure in {fi}:{pi}"
                ));
            }
        }
        Op::GetField { dst, base, .. } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *base)?;
        }
        Op::SwitchTag {
            scrutinee,
            arms,
            default,
        } => {
            reg_ok(f, *scrutinee)?;
            jump_ok(*default)?;
            for (_, t) in arms {
                jump_ok(*t)?;
            }
        }
        Op::MakeBitArray { dst, values, specs } => {
            reg_ok(f, *dst)?;
            if values.len() != specs.len() {
                return Err(format!(
                    "MakeBitArray value/spec length mismatch in {fi}:{pi}"
                ));
            }
            for a in values {
                reg_ok(f, *a)?;
            }
        }
        Op::BitArrayTakeInt {
            ok,
            value,
            rest,
            src,
            ..
        } => {
            reg_ok(f, *ok)?;
            reg_ok(f, *value)?;
            reg_ok(f, *rest)?;
            reg_ok(f, *src)?;
        }
        Op::BitArrayTakeUtf8 {
            ok,
            rest,
            src,
            expected,
        } => {
            reg_ok(f, *ok)?;
            reg_ok(f, *rest)?;
            reg_ok(f, *src)?;
            reg_ok(f, *expected)?;
        }
        Op::BitArrayTakeRest { ok, value, src, .. } => {
            reg_ok(f, *ok)?;
            reg_ok(f, *value)?;
            reg_ok(f, *src)?;
        }
        Op::BitArrayIsEmpty { dst, src } => {
            reg_ok(f, *dst)?;
            reg_ok(f, *src)?;
        }
        Op::StringTakePrefix {
            ok,
            rest,
            src,
            expected,
        } => {
            reg_ok(f, *ok)?;
            reg_ok(f, *rest)?;
            reg_ok(f, *src)?;
            reg_ok(f, *expected)?;
        }
    }
    Ok(())
}
