//! Bump-arena allocator (no reclamation until step 4).
use crate::value::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapPtr(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind { BoxedInt, BoxedFloat, String, Tuple, Cons, EmptyList, Adt, Closure, BitArray }
enum Object {
    BoxedInt(i64), BoxedFloat(f64), String(Vec<u8>), Tuple(Vec<Value>),
    Cons { head: Value, tail: Value }, EmptyList,
    Adt { type_tag: u16, variant: u16, fields: Vec<Value> },
    #[allow(dead_code)] Closure { func: u32, captures: Vec<Value> },
    #[allow(dead_code)] BitArray { bits: Vec<u8>, bit_len: u64 },
}
pub struct Heap { objects: Vec<Object>, pub words_allocated: u64, pub objects_allocated: u64 }
impl Default for Heap { fn default() -> Self { Self::new() } }
impl Heap {
    pub fn new() -> Self { Self { objects: Vec::with_capacity(256), words_allocated: 0, objects_allocated: 0 } }
    fn push(&mut self, obj: Object, words: u64) -> HeapPtr {
        let idx = self.objects.len() as u64; self.objects.push(obj);
        self.objects_allocated += 1; self.words_allocated += words; HeapPtr(idx << 3)
    }
    fn get(&self, p: HeapPtr) -> &Object { &self.objects[(p.0 >> 3) as usize] }
    pub fn kind(&self, p: HeapPtr) -> ObjectKind {
        match self.get(p) {
            Object::BoxedInt(_) => ObjectKind::BoxedInt, Object::BoxedFloat(_) => ObjectKind::BoxedFloat,
            Object::String(_) => ObjectKind::String, Object::Tuple(_) => ObjectKind::Tuple,
            Object::Cons { .. } => ObjectKind::Cons, Object::EmptyList => ObjectKind::EmptyList,
            Object::Adt { .. } => ObjectKind::Adt, Object::Closure { .. } => ObjectKind::Closure,
            Object::BitArray { .. } => ObjectKind::BitArray,
        }
    }
    pub fn alloc_boxed_int(&mut self, v: i64) -> HeapPtr { self.push(Object::BoxedInt(v), 2) }
    pub fn boxed_int(&self, p: HeapPtr) -> i64 { match self.get(p) { Object::BoxedInt(v) => *v, _ => 0 } }
    pub fn alloc_boxed_float(&mut self, v: f64) -> HeapPtr { self.push(Object::BoxedFloat(v), 2) }
    pub fn boxed_float(&self, p: HeapPtr) -> f64 { match self.get(p) { Object::BoxedFloat(v) => *v, _ => 0.0 } }
    pub fn alloc_string(&mut self, bytes: &[u8]) -> HeapPtr { self.push(Object::String(bytes.to_vec()), 2 + ((bytes.len() as u64)+7)/8) }
    pub fn string_bytes(&self, p: HeapPtr) -> &[u8] { match self.get(p) { Object::String(b) => b, _ => &[] } }
    pub fn alloc_tuple(&mut self, fields: &[Value]) -> HeapPtr { self.push(Object::Tuple(fields.to_vec()), 2 + fields.len() as u64) }
    pub fn tuple_field(&self, p: HeapPtr, i: usize) -> Value { match self.get(p) { Object::Tuple(fs) => fs[i], _ => Value::nil() } }
    pub fn alloc_cons(&mut self, head: Value, tail: Value) -> HeapPtr { self.push(Object::Cons { head, tail }, 3) }
    pub fn cons_head(&self, p: HeapPtr) -> Value { match self.get(p) { Object::Cons { head, .. } => *head, _ => Value::nil() } }
    pub fn cons_tail(&self, p: HeapPtr) -> Value { match self.get(p) { Object::Cons { tail, .. } => *tail, _ => Value::nil() } }
    pub fn empty_list(&mut self) -> HeapPtr { self.push(Object::EmptyList, 1) }
    pub fn alloc_adt(&mut self, type_tag: u16, variant: u16, fields: &[Value]) -> HeapPtr {
        self.push(Object::Adt { type_tag, variant, fields: fields.to_vec() }, 3 + fields.len() as u64)
    }
    pub fn adt_variant(&self, p: HeapPtr) -> u16 { match self.get(p) { Object::Adt { variant, .. } => *variant, _ => 0 } }
    pub fn adt_field(&self, p: HeapPtr, i: usize) -> Value { match self.get(p) { Object::Adt { fields, .. } => fields[i], _ => Value::nil() } }
}
