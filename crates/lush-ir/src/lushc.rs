//! Deterministic `.lushc` encode/decode (issue #24 §8).
//!
//! Format (little-endian):
//! - magic: b"LUSH"
//! - version: u32 = 1
//! - n_constants: u32
//! - constants…
//! - n_functions: u32
//! - functions…
//! - entry: u32
//!
//! Strings are `u32` length + UTF-8 bytes. Ops are tagged with a `u16` discriminant.

use crate::bytecode::{BitSegEnc, Builtin, Constant, Function, Op, Program};

pub const MAGIC: &[u8; 4] = b"LUSH";
pub const VERSION: u32 = 2;

pub fn encode(program: &Program) -> Vec<u8> {
    let mut w = Writer::default();
    w.bytes(MAGIC);
    w.u32(VERSION);
    w.u32(program.constants.len() as u32);
    for c in &program.constants {
        encode_const(&mut w, c);
    }
    w.u32(program.functions.len() as u32);
    for f in &program.functions {
        encode_function(&mut w, f);
    }
    w.u32(program.entry);
    // sources omitted from .lushc for determinism of code units; debug lines are per-op.
    w.buf
}

pub fn decode(bytes: &[u8]) -> Result<Program, String> {
    let mut r = Reader {
        data: bytes,
        pos: 0,
    };
    let magic = r.bytes(4)?;
    if magic != MAGIC {
        return Err("bad magic".into());
    }
    let ver = r.u32()?;
    if ver != VERSION {
        return Err(format!("unsupported lushc version {ver}"));
    }
    let n_c = r.u32()? as usize;
    let mut constants = Vec::with_capacity(n_c);
    for _ in 0..n_c {
        constants.push(decode_const(&mut r)?);
    }
    let n_f = r.u32()? as usize;
    let mut functions = Vec::with_capacity(n_f);
    for _ in 0..n_f {
        functions.push(decode_function(&mut r)?);
    }
    let entry = r.u32()?;
    let program = Program {
        functions,
        constants,
        entry,
        sources: Default::default(),
    };
    program.verify().map_err(|e| format!("verify: {e}"))?;
    Ok(program)
}

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}
impl Writer {
    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }
    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.bytes(s.as_bytes());
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("truncated lushc".into());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(self.u64()? as i64)
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let b = self.bytes(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| "invalid utf8".into())
    }
}

fn encode_const(w: &mut Writer, c: &Constant) {
    match c {
        Constant::Int(n) => {
            w.u8(1);
            w.i64(*n);
        }
        Constant::Float(f) => {
            w.u8(2);
            w.f64(*f);
        }
        Constant::String(s) => {
            w.u8(3);
            w.str(s);
        }
        Constant::Nullary { type_tag, variant } => {
            w.u8(4);
            w.u16(*type_tag);
            w.u16(*variant);
        }
    }
}

fn decode_const(r: &mut Reader<'_>) -> Result<Constant, String> {
    match r.u8()? {
        1 => Ok(Constant::Int(r.i64()?)),
        2 => Ok(Constant::Float(r.f64()?)),
        3 => Ok(Constant::String(r.str()?)),
        4 => Ok(Constant::Nullary {
            type_tag: r.u16()?,
            variant: r.u16()?,
        }),
        t => Err(format!("unknown const tag {t}")),
    }
}

fn encode_function(w: &mut Writer, f: &Function) {
    w.str(&f.module);
    w.str(&f.name);
    w.u8(f.arity);
    w.u8(f.n_captures);
    w.u16(f.regs);
    w.str(&f.source_path);
    w.u32(f.code.len() as u32);
    for (op, (line, col)) in f
        .code
        .iter()
        .zip(f.lines.iter().chain(std::iter::repeat(&(0, 0))))
    {
        encode_op(w, op);
        w.u32(*line);
        w.u32(*col);
    }
}

fn decode_function(r: &mut Reader<'_>) -> Result<Function, String> {
    let module = r.str()?;
    let name = r.str()?;
    let arity = r.u8()?;
    let n_captures = r.u8()?;
    let regs = r.u16()?;
    let source_path = r.str()?;
    let n = r.u32()? as usize;
    let mut code = Vec::with_capacity(n);
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        code.push(decode_op(r)?);
        lines.push((r.u32()?, r.u32()?));
    }
    Ok(Function {
        module,
        name,
        arity,
        n_captures,
        regs,
        code,
        lines,
        source_path,
    })
}

fn encode_regs(w: &mut Writer, regs: &[u8]) {
    w.u16(regs.len() as u16);
    for r in regs {
        w.u8(*r);
    }
}

fn decode_regs(r: &mut Reader<'_>) -> Result<Vec<u8>, String> {
    let n = r.u16()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(r.u8()?);
    }
    Ok(out)
}

fn encode_builtin(w: &mut Writer, b: Builtin) {
    w.u8(match b {
        Builtin::Print => 1,
        Builtin::Println => 2,
        Builtin::Eprintln => 3,
        Builtin::IntToString => 4,
    });
}

fn decode_builtin(r: &mut Reader<'_>) -> Result<Builtin, String> {
    match r.u8()? {
        1 => Ok(Builtin::Print),
        2 => Ok(Builtin::Println),
        3 => Ok(Builtin::Eprintln),
        4 => Ok(Builtin::IntToString),
        t => Err(format!("unknown builtin {t}")),
    }
}

fn encode_op(w: &mut Writer, op: &Op) {
    match op {
        Op::Move { dst, src } => {
            w.u16(1);
            w.u8(*dst);
            w.u8(*src);
        }
        Op::LoadConst { dst, idx } => {
            w.u16(2);
            w.u8(*dst);
            w.u32(*idx);
        }
        Op::LoadInt { dst, value } => {
            w.u16(3);
            w.u8(*dst);
            w.i64(*value);
        }
        Op::LoadBool { dst, value } => {
            w.u16(4);
            w.u8(*dst);
            w.u8(u8::from(*value));
        }
        Op::LoadNil { dst } => {
            w.u16(5);
            w.u8(*dst);
        }
        Op::Add { dst, a, b } => {
            w.u16(10);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Sub { dst, a, b } => {
            w.u16(11);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Mul { dst, a, b } => {
            w.u16(12);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Div { dst, a, b } => {
            w.u16(13);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Rem { dst, a, b } => {
            w.u16(14);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Neg { dst, src } => {
            w.u16(15);
            w.u8(*dst);
            w.u8(*src);
        }
        Op::Concat { dst, a, b } => {
            w.u16(16);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Eq { dst, a, b } => {
            w.u16(20);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Ne { dst, a, b } => {
            w.u16(21);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Lt { dst, a, b } => {
            w.u16(22);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Le { dst, a, b } => {
            w.u16(23);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Gt { dst, a, b } => {
            w.u16(24);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Ge { dst, a, b } => {
            w.u16(25);
            w.u8(*dst);
            w.u8(*a);
            w.u8(*b);
        }
        Op::Jump { target } => {
            w.u16(30);
            w.u32(*target);
        }
        Op::JumpIfFalse { cond, target } => {
            w.u16(31);
            w.u8(*cond);
            w.u32(*target);
        }
        Op::Call { dst, func, args } => {
            w.u16(40);
            w.u8(*dst);
            w.u32(*func);
            encode_regs(w, args);
        }
        Op::TailCall { func, args } => {
            w.u16(41);
            w.u32(*func);
            encode_regs(w, args);
        }
        Op::CallClosure { dst, clo, args } => {
            w.u16(42);
            w.u8(*dst);
            w.u8(*clo);
            encode_regs(w, args);
        }
        Op::TailCallClosure { clo, args } => {
            w.u16(43);
            w.u8(*clo);
            encode_regs(w, args);
        }
        Op::Return { src } => {
            w.u16(44);
            w.u8(*src);
        }
        Op::Panic { msg } => {
            w.u16(45);
            w.u32(*msg);
        }
        Op::Builtin { dst, builtin, args } => {
            w.u16(46);
            w.u8(*dst);
            encode_builtin(w, *builtin);
            encode_regs(w, args);
        }
        Op::MakeTuple { dst, fields } => {
            w.u16(50);
            w.u8(*dst);
            encode_regs(w, fields);
        }
        Op::MakeCons { dst, head, tail } => {
            w.u16(51);
            w.u8(*dst);
            w.u8(*head);
            w.u8(*tail);
        }
        Op::MakeEmptyList { dst } => {
            w.u16(52);
            w.u8(*dst);
        }
        Op::MakeClosure {
            dst,
            func,
            captures,
        } => {
            w.u16(59);
            w.u8(*dst);
            w.u32(*func);
            encode_regs(w, captures);
        }
        Op::MakeAdt {
            dst,
            type_tag,
            variant,
            fields,
        } => {
            w.u16(53);
            w.u8(*dst);
            w.u16(*type_tag);
            w.u16(*variant);
            encode_regs(w, fields);
        }
        Op::GetField { dst, base, index } => {
            w.u16(54);
            w.u8(*dst);
            w.u8(*base);
            w.u16(*index);
        }
        Op::SwitchTag {
            scrutinee,
            arms,
            default,
        } => {
            w.u16(55);
            w.u8(*scrutinee);
            w.u16(arms.len() as u16);
            for (tag, pc) in arms {
                w.u16(*tag);
                w.u32(*pc);
            }
            w.u32(*default);
        }
        Op::IsInt { dst, src, value } => {
            w.u16(56);
            w.u8(*dst);
            w.u8(*src);
            w.i64(*value);
        }
        Op::IsEmptyList { dst, src } => {
            w.u16(57);
            w.u8(*dst);
            w.u8(*src);
        }
        Op::Echo { dst, src } => {
            w.u16(58);
            w.u8(*dst);
            w.u8(*src);
        }
        Op::MakeBitArray { dst, values, specs } => {
            w.u16(60);
            w.u8(*dst);
            encode_regs(w, values);
            w.u16(specs.len() as u16);
            for s in specs {
                encode_bit_seg(w, s);
            }
        }
        Op::BitArrayTakeInt {
            ok,
            value,
            rest,
            src,
            size_reg,
            signed,
            little,
        } => {
            w.u16(61);
            w.u8(*ok);
            w.u8(*value);
            w.u8(*rest);
            w.u8(*src);
            w.u8(*size_reg);
            let mut flags = 0u8;
            if *signed {
                flags |= 1;
            }
            if *little {
                flags |= 2;
            }
            w.u8(flags);
        }
        Op::BitArrayTakeUtf8 {
            ok,
            rest,
            src,
            expected,
        } => {
            w.u16(62);
            w.u8(*ok);
            w.u8(*rest);
            w.u8(*src);
            w.u8(*expected);
        }
        Op::BitArrayTakeSlice {
            ok,
            value,
            rest,
            src,
            size_reg,
            unit_is_bytes,
            require_byte_aligned,
        } => {
            w.u16(66);
            w.u8(*ok);
            w.u8(*value);
            w.u8(*rest);
            w.u8(*src);
            w.u8(*size_reg);
            let mut flags = 0u8;
            if *unit_is_bytes {
                flags |= 1;
            }
            if *require_byte_aligned {
                flags |= 2;
            }
            w.u8(flags);
        }
        Op::BitArrayTakeRest {
            ok,
            value,
            src,
            require_byte_aligned,
        } => {
            w.u16(63);
            w.u8(*ok);
            w.u8(*value);
            w.u8(*src);
            w.u8(u8::from(*require_byte_aligned));
        }
        Op::BitArrayIsEmpty { dst, src } => {
            w.u16(64);
            w.u8(*dst);
            w.u8(*src);
        }
        Op::StringTakePrefix {
            ok,
            rest,
            src,
            expected,
        } => {
            w.u16(65);
            w.u8(*ok);
            w.u8(*rest);
            w.u8(*src);
            w.u8(*expected);
        }
    }
}

fn encode_bit_seg(w: &mut Writer, s: &BitSegEnc) {
    match s {
        BitSegEnc::Int {
            size,
            signed,
            little,
        } => {
            w.u8(0);
            w.u8(*size);
            let mut flags = 0u8;
            if *signed {
                flags |= 1;
            }
            if *little {
                flags |= 2;
            }
            w.u8(flags);
        }
        BitSegEnc::Utf8 => w.u8(1),
        BitSegEnc::Bits { size_bits } => {
            w.u8(2);
            match size_bits {
                Some(n) => {
                    w.u8(1);
                    w.u32(*n);
                }
                None => w.u8(0),
            }
        }
        BitSegEnc::Bytes { size_bytes } => {
            w.u8(3);
            match size_bytes {
                Some(n) => {
                    w.u8(1);
                    w.u32(*n);
                }
                None => w.u8(0),
            }
        }
    }
}

fn decode_bit_seg(r: &mut Reader<'_>) -> Result<BitSegEnc, String> {
    match r.u8()? {
        0 => {
            let size = r.u8()?;
            let flags = r.u8()?;
            Ok(BitSegEnc::Int {
                size,
                signed: flags & 1 != 0,
                little: flags & 2 != 0,
            })
        }
        1 => Ok(BitSegEnc::Utf8),
        2 => {
            let has = r.u8()?;
            let size_bits = if has != 0 { Some(r.u32()?) } else { None };
            Ok(BitSegEnc::Bits { size_bits })
        }
        3 => {
            let has = r.u8()?;
            let size_bytes = if has != 0 { Some(r.u32()?) } else { None };
            Ok(BitSegEnc::Bytes { size_bytes })
        }
        t => Err(format!("unknown bit-seg tag {t}")),
    }
}

fn decode_op(r: &mut Reader<'_>) -> Result<Op, String> {
    match r.u16()? {
        1 => Ok(Op::Move {
            dst: r.u8()?,
            src: r.u8()?,
        }),
        2 => Ok(Op::LoadConst {
            dst: r.u8()?,
            idx: r.u32()?,
        }),
        3 => Ok(Op::LoadInt {
            dst: r.u8()?,
            value: r.i64()?,
        }),
        4 => Ok(Op::LoadBool {
            dst: r.u8()?,
            value: r.u8()? != 0,
        }),
        5 => Ok(Op::LoadNil { dst: r.u8()? }),
        10 => Ok(Op::Add {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        11 => Ok(Op::Sub {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        12 => Ok(Op::Mul {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        13 => Ok(Op::Div {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        14 => Ok(Op::Rem {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        15 => Ok(Op::Neg {
            dst: r.u8()?,
            src: r.u8()?,
        }),
        16 => Ok(Op::Concat {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        20 => Ok(Op::Eq {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        21 => Ok(Op::Ne {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        22 => Ok(Op::Lt {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        23 => Ok(Op::Le {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        24 => Ok(Op::Gt {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        25 => Ok(Op::Ge {
            dst: r.u8()?,
            a: r.u8()?,
            b: r.u8()?,
        }),
        30 => Ok(Op::Jump { target: r.u32()? }),
        31 => Ok(Op::JumpIfFalse {
            cond: r.u8()?,
            target: r.u32()?,
        }),
        40 => Ok(Op::Call {
            dst: r.u8()?,
            func: r.u32()?,
            args: decode_regs(r)?,
        }),
        41 => Ok(Op::TailCall {
            func: r.u32()?,
            args: decode_regs(r)?,
        }),
        42 => Ok(Op::CallClosure {
            dst: r.u8()?,
            clo: r.u8()?,
            args: decode_regs(r)?,
        }),
        43 => Ok(Op::TailCallClosure {
            clo: r.u8()?,
            args: decode_regs(r)?,
        }),
        44 => Ok(Op::Return { src: r.u8()? }),
        45 => Ok(Op::Panic { msg: r.u32()? }),
        46 => Ok(Op::Builtin {
            dst: r.u8()?,
            builtin: decode_builtin(r)?,
            args: decode_regs(r)?,
        }),
        50 => Ok(Op::MakeTuple {
            dst: r.u8()?,
            fields: decode_regs(r)?,
        }),
        51 => Ok(Op::MakeCons {
            dst: r.u8()?,
            head: r.u8()?,
            tail: r.u8()?,
        }),
        52 => Ok(Op::MakeEmptyList { dst: r.u8()? }),
        59 => Ok(Op::MakeClosure {
            dst: r.u8()?,
            func: r.u32()?,
            captures: decode_regs(r)?,
        }),
        53 => Ok(Op::MakeAdt {
            dst: r.u8()?,
            type_tag: r.u16()?,
            variant: r.u16()?,
            fields: decode_regs(r)?,
        }),
        54 => Ok(Op::GetField {
            dst: r.u8()?,
            base: r.u8()?,
            index: r.u16()?,
        }),
        55 => {
            let scrutinee = r.u8()?;
            let n = r.u16()? as usize;
            let mut arms = Vec::with_capacity(n);
            for _ in 0..n {
                arms.push((r.u16()?, r.u32()?));
            }
            Ok(Op::SwitchTag {
                scrutinee,
                arms,
                default: r.u32()?,
            })
        }
        56 => Ok(Op::IsInt {
            dst: r.u8()?,
            src: r.u8()?,
            value: r.i64()?,
        }),
        57 => Ok(Op::IsEmptyList {
            dst: r.u8()?,
            src: r.u8()?,
        }),
        58 => Ok(Op::Echo {
            dst: r.u8()?,
            src: r.u8()?,
        }),
        60 => {
            let dst = r.u8()?;
            let values = decode_regs(r)?;
            let n = r.u16()? as usize;
            let mut specs = Vec::with_capacity(n);
            for _ in 0..n {
                specs.push(decode_bit_seg(r)?);
            }
            Ok(Op::MakeBitArray { dst, values, specs })
        }
        61 => {
            let ok = r.u8()?;
            let value = r.u8()?;
            let rest = r.u8()?;
            let src = r.u8()?;
            let size_reg = r.u8()?;
            let flags = r.u8()?;
            Ok(Op::BitArrayTakeInt {
                ok,
                value,
                rest,
                src,
                size_reg,
                signed: flags & 1 != 0,
                little: flags & 2 != 0,
            })
        }
        62 => Ok(Op::BitArrayTakeUtf8 {
            ok: r.u8()?,
            rest: r.u8()?,
            src: r.u8()?,
            expected: r.u8()?,
        }),
        63 => Ok(Op::BitArrayTakeRest {
            ok: r.u8()?,
            value: r.u8()?,
            src: r.u8()?,
            require_byte_aligned: r.u8()? != 0,
        }),
        64 => Ok(Op::BitArrayIsEmpty {
            dst: r.u8()?,
            src: r.u8()?,
        }),
        65 => Ok(Op::StringTakePrefix {
            ok: r.u8()?,
            rest: r.u8()?,
            src: r.u8()?,
            expected: r.u8()?,
        }),
        66 => {
            let ok = r.u8()?;
            let value = r.u8()?;
            let rest = r.u8()?;
            let src = r.u8()?;
            let size_reg = r.u8()?;
            let flags = r.u8()?;
            Ok(Op::BitArrayTakeSlice {
                ok,
                value,
                rest,
                src,
                size_reg,
                unit_is_bytes: flags & 1 != 0,
                require_byte_aligned: flags & 2 != 0,
            })
        }
        t => Err(format!("unknown opcode {t}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_source;

    #[test]
    fn lushc_roundtrip_fib_deterministic() {
        let src = include_str!("../../../tests/e2e/fib.lush");
        let p1 = compile_source("main", src).unwrap();
        let b1 = encode(&p1);
        let b2 = encode(&p1);
        assert_eq!(b1, b2);
        let p2 = decode(&b1).unwrap();
        assert_eq!(p2.functions.len(), p1.functions.len());
        assert_eq!(p2.entry, p1.entry);
        assert_eq!(encode(&p2), b1);
    }
}
