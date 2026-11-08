use super::*;
use crate::bytecode::value::JitVecSlot;
use crate::jit::trace::{Operand, SpecializedOpKind};
use crate::number::{LustFloat, LustInt};

fn vec_kind(layout: &SpecializedLayout) -> Option<VecKind> {
    match layout {
        SpecializedLayout::Vec { element_layout, element_size } => match &**element_layout {
            SpecializedLayout::Scalar { size: 1, .. } => Some(VecKind::Bool),
            SpecializedLayout::Scalar { size, is_float: false, .. }
                if *size == core::mem::size_of::<LustInt>() =>
            {
                Some(VecKind::Int)
            }
            SpecializedLayout::Scalar { size, is_float: true, .. }
                if *size == core::mem::size_of::<LustFloat>() =>
            {
                Some(VecKind::Float)
            }
            _ if *element_size == 1 => Some(VecKind::Bool),
            _ => None,
        },
        _ => None,
    }
}

impl JitCompiler {
    pub(super) fn slot_offset(&self, specialized_id: usize, what: &str) -> Result<i32> {
        self.specialized_values
            .get(&specialized_id)
            .map(|value| value.stack_offset)
            .ok_or_else(|| crate::LustError::RuntimeError {
                message: format!("Specialized {} #{} not found", what, specialized_id),
            })
    }

    /// Compile Unbox operation - convert Value to specialized representation
    pub(super) fn compile_unbox(
        &mut self,
        specialized_id: usize,
        source_reg: u8,
        layout: &SpecializedLayout,
        guard_index: &mut i32,
        guards: &mut Vec<Guard>,
    ) -> Result<()> {
        match vec_kind(layout) {
            Some(VecKind::Int) => {
                self.compile_unbox_array_int(specialized_id, source_reg, guard_index, guards)
            }
            Some(VecKind::Bool) => {
                self.compile_unbox_array_bool(specialized_id, source_reg, guard_index, guards)
            }
            Some(VecKind::Float) => {
                self.compile_unbox_array_float(specialized_id, source_reg, guard_index, guards)
            }
            None => Err(crate::LustError::RuntimeError {
                message: format!("Unbox not yet implemented for layout {:?}", layout),
            }),
        }
    }

    /// Compile Rebox operation - convert specialized representation back to Value
    pub(super) fn compile_rebox(
        &mut self,
        dest_reg: u8,
        specialized_id: usize,
        layout: &SpecializedLayout,
    ) -> Result<()> {
        match vec_kind(layout) {
            Some(VecKind::Int) => self.compile_rebox_array_int(dest_reg, specialized_id),
            Some(VecKind::Bool) => self.compile_rebox_array_bool(dest_reg, specialized_id),
            Some(VecKind::Float) => self.compile_rebox_array_float(dest_reg, specialized_id),
            None => Err(crate::LustError::RuntimeError {
                message: format!("Rebox not yet implemented for layout {:?}", layout),
            }),
        }
    }

    /// Compile specialized operation
    pub(super) fn compile_specialized_op(
        &mut self,
        op: &SpecializedOpKind,
        operands: &[Operand],
        is_last_push: bool,
    ) -> Result<()> {
        match op {
            SpecializedOpKind::VecPush => {
                if operands.len() != 2 {
                    return Err(crate::LustError::RuntimeError {
                        message: "VecPush requires 2 operands (vec_id, value)".into(),
                    });
                }
                match (&operands[0], &operands[1]) {
                    (Operand::Specialized(vec_id), Operand::Register(value_reg)) => {
                        let kind = self
                            .specialized_values
                            .get(vec_id)
                            .map(|s| s.kind)
                            .unwrap_or(VecKind::Int);
                        match kind {
                            VecKind::Int => self.compile_vec_int_push(*vec_id, *value_reg, is_last_push)?,
                            VecKind::Bool => self.compile_vec_bool_push(*vec_id, *value_reg, is_last_push)?,
                            VecKind::Float => self.compile_vec_float_push(*vec_id, *value_reg)?,
                        }
                    }
                    _ => {
                        return Err(crate::LustError::RuntimeError {
                            message: "VecPush operands must be (Specialized, Register)".into(),
                        });
                    }
                }
            }
            SpecializedOpKind::VecLen => {
                if operands.len() != 2 {
                    return Err(crate::LustError::RuntimeError {
                        message: "VecLen requires 2 operands (vec_id, dest_reg)".into(),
                    });
                }
                match (&operands[0], &operands[1]) {
                    (Operand::Specialized(vec_id), Operand::Register(dest_reg)) => {
                        self.compile_vec_int_len(*vec_id, *dest_reg)?;
                    }
                    _ => {
                        return Err(crate::LustError::RuntimeError {
                            message: "VecLen operands must be (Specialized, Register)".into(),
                        });
                    }
                }
            }
            SpecializedOpKind::VecSet => {
                if operands.len() != 3 {
                    return Err(crate::LustError::RuntimeError {
                        message: "VecSet requires 3 operands (vec_id, index, value)".into(),
                    });
                }
                match (&operands[0], &operands[1], &operands[2]) {
                    (
                        Operand::Specialized(vec_id),
                        Operand::Register(index_reg),
                        Operand::Register(value_reg),
                    ) => {
                        self.compile_vec_set(*vec_id, *index_reg, *value_reg)?;
                    }
                    _ => {
                        return Err(crate::LustError::RuntimeError {
                            message: "VecSet operands must be (Specialized, Register, Register)"
                                .into(),
                        });
                    }
                }
            }
            SpecializedOpKind::VecGet => {
                if operands.len() != 3 {
                    return Err(crate::LustError::RuntimeError {
                        message: "VecGet requires 3 operands (dest_reg, vec_id, index)".into(),
                    });
                }
                match (&operands[0], &operands[1], &operands[2]) {
                    (
                        Operand::Register(dest_reg),
                        Operand::Specialized(vec_id),
                        Operand::Register(index_reg),
                    ) => {
                        self.compile_vec_get(*dest_reg, *vec_id, *index_reg)?;
                    }
                    _ => {
                        return Err(crate::LustError::RuntimeError {
                            message: "VecGet operands must be (Register, Specialized, Register)"
                                .into(),
                        });
                    }
                }
            }
            SpecializedOpKind::VecIndexOk => {
                if operands.len() != 4 {
                    return Err(crate::LustError::RuntimeError {
                        message: "VecIndexOk requires 4 operands (value_dest, condition_dest, vec_id, index)".into(),
                    });
                }
                match (&operands[0], &operands[1], &operands[2], &operands[3]) {
                    (
                        Operand::Register(value_dest),
                        Operand::Register(condition_dest),
                        Operand::Specialized(vec_id),
                        Operand::Register(index_reg),
                    ) => {
                        self.compile_vec_index_ok(*value_dest, *condition_dest, *vec_id, *index_reg)?;
                    }
                    _ => {
                        return Err(crate::LustError::RuntimeError {
                            message: "VecIndexOk operands must be (Register, Register, Specialized, Register)"
                                .into(),
                        });
                    }
                }
            }
            _ => {
                return Err(crate::LustError::RuntimeError {
                    message: format!("Specialized op {:?} not yet implemented", op),
                });
            }
        }
        Ok(())
    }

    /// Unbox Array<int> - convert Rc<RefCell<Vec<Value>>> to Vec<LustInt> on stack
    fn compile_unbox_array_int(
        &mut self,
        specialized_id: usize,
        source_reg: u8,
        guard_index: &mut i32,
        guards: &mut Vec<Guard>,
    ) -> Result<()> {
        jit::log(|| {
            format!(
                "📦 JIT: Unboxing Array<int> from reg {} to specialized #{}",
                source_reg, specialized_id
            )
        });

        let stack_offset = match self.specialized_values.get(&specialized_id) {
            Some(existing) => existing.stack_offset,
            None => {
                let stack_offset = self.allocate_specialized_stack(32, 8);
                self.specialized_values.insert(
                    specialized_id,
                    SpecializedValue {
                        stack_offset,
                        kind: VecKind::Int,
                    },
                );
                stack_offset
            }
        };

        unsafe extern "C" {
            fn jit_unbox_array_int(array_value_ptr: *const Value, slot: *mut JitVecSlot) -> u8;
        }
        self.emit_reg_addr(0, source_reg);
        self.emit_slot_addr(1, stack_offset);
        self.emit_call(jit_unbox_array_int as *const ());

        let guard_idx = *guard_index as usize;
        *guard_index += 1;
        guards.push(Guard {
            index: guard_idx,
            bailout_ip: self.guard_bailout_ip(),
            kind: GuardKind::Plain { register: source_reg },
            fail_count: 0,
        });
        let guard_return_value = (guard_idx + 1) as i32;
        dynasm!(self.ops
            ; .arch aarch64
            ; and w0, w0, 0xff
            ; cmp w0, 1
            ; b.eq >unbox_int_ok
        );
        self.emit_guard_exit(guard_return_value);
        dynasm!(self.ops
            ; .arch aarch64
            ; unbox_int_ok:
        );

        Ok(())
    }

    /// Unbox Array<bool> - convert Rc<RefCell<Vec<Value>>> to Vec<u8> on stack
    fn compile_unbox_array_bool(
        &mut self,
        specialized_id: usize,
        source_reg: u8,
        guard_index: &mut i32,
        guards: &mut Vec<Guard>,
    ) -> Result<()> {
        jit::log(|| {
            format!(
                "📦 JIT: Unboxing Array<bool> from reg {} to specialized #{}",
                source_reg, specialized_id
            )
        });

        let stack_offset = match self.specialized_values.get(&specialized_id) {
            Some(existing) => existing.stack_offset,
            None => {
                let stack_offset = self.allocate_specialized_stack(32, 8);
                self.specialized_values.insert(
                    specialized_id,
                    SpecializedValue {
                        stack_offset,
                        kind: VecKind::Bool,
                    },
                );
                stack_offset
            }
        };

        unsafe extern "C" {
            fn jit_unbox_array_bool(array_value_ptr: *const Value, slot: *mut JitVecSlot) -> u8;
        }
        self.emit_reg_addr(0, source_reg);
        self.emit_slot_addr(1, stack_offset);
        self.emit_call(jit_unbox_array_bool as *const ());

        let guard_idx = *guard_index as usize;
        *guard_index += 1;
        guards.push(Guard {
            index: guard_idx,
            bailout_ip: self.guard_bailout_ip(),
            kind: GuardKind::Plain { register: source_reg },
            fail_count: 0,
        });
        let guard_return_value = (guard_idx + 1) as i32;
        dynasm!(self.ops
            ; .arch aarch64
            ; and w0, w0, 0xff
            ; cmp w0, 1
            ; b.eq >unbox_bool_ok
        );
        self.emit_guard_exit(guard_return_value);
        dynasm!(self.ops
            ; .arch aarch64
            ; unbox_bool_ok:
        );

        Ok(())
    }

    /// Unbox Array<float> - convert Rc<RefCell<Vec<Value>>> to Vec<LustFloat> on stack
    fn compile_unbox_array_float(
        &mut self,
        specialized_id: usize,
        source_reg: u8,
        guard_index: &mut i32,
        guards: &mut Vec<Guard>,
    ) -> Result<()> {
        jit::log(|| {
            format!(
                "📦 JIT: Unboxing Array<float> from reg {} to specialized #{}",
                source_reg, specialized_id
            )
        });

        let stack_offset = match self.specialized_values.get(&specialized_id) {
            Some(existing) => existing.stack_offset,
            None => {
                let stack_offset = self.allocate_specialized_stack(32, 8);
                self.specialized_values.insert(
                    specialized_id,
                    SpecializedValue {
                        stack_offset,
                        kind: VecKind::Float,
                    },
                );
                stack_offset
            }
        };

        unsafe extern "C" {
            fn jit_unbox_array_float(array_value_ptr: *const Value, slot: *mut JitVecSlot) -> u8;
        }
        self.emit_reg_addr(0, source_reg);
        self.emit_slot_addr(1, stack_offset);
        self.emit_call(jit_unbox_array_float as *const ());

        let guard_idx = *guard_index as usize;
        *guard_index += 1;
        guards.push(Guard {
            index: guard_idx,
            bailout_ip: self.guard_bailout_ip(),
            kind: GuardKind::Plain { register: source_reg },
            fail_count: 0,
        });
        let guard_return_value = (guard_idx + 1) as i32;
        dynasm!(self.ops
            ; .arch aarch64
            ; and w0, w0, 0xff
            ; cmp w0, 1
            ; b.eq >unbox_float_ok
        );
        self.emit_guard_exit(guard_return_value);
        dynasm!(self.ops
            ; .arch aarch64
            ; unbox_float_ok:
        );

        Ok(())
    }

    /// Rebox Array<int> - convert Vec<LustInt> back to Rc<RefCell<Vec<Value>>>
    fn compile_rebox_array_int(&mut self, dest_reg: u8, specialized_id: usize) -> Result<()> {
        jit::log(|| {
            format!(
                "📦 JIT: Reboxing specialized #{} to Array<int> in reg {}",
                specialized_id, dest_reg
            )
        });

        let stack_offset = self.slot_offset(specialized_id, "value")?;
        let _ = dest_reg;
        unsafe extern "C" {
            fn jit_rebox_array_int(slot: *mut JitVecSlot) -> u8;
        }
        self.emit_slot_addr(0, stack_offset);
        self.emit_call(jit_rebox_array_int as *const ());
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    /// Rebox Array<bool> - convert Vec<u8> back to Rc<RefCell<Vec<Value>>>
    fn compile_rebox_array_bool(&mut self, dest_reg: u8, specialized_id: usize) -> Result<()> {
        jit::log(|| {
            format!(
                "📦 JIT: Reboxing specialized #{} to Array<bool> in reg {}",
                specialized_id, dest_reg
            )
        });

        let stack_offset = self.slot_offset(specialized_id, "value")?;
        let _ = dest_reg;
        unsafe extern "C" {
            fn jit_rebox_array_bool(slot: *mut JitVecSlot) -> u8;
        }
        self.emit_slot_addr(0, stack_offset);
        self.emit_call(jit_rebox_array_bool as *const ());
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    /// Rebox Array<float> - convert Vec<LustFloat> back to Rc<RefCell<Vec<Value>>>
    fn compile_rebox_array_float(&mut self, dest_reg: u8, specialized_id: usize) -> Result<()> {
        jit::log(|| {
            format!(
                "📦 JIT: Reboxing specialized #{} to Array<float> in reg {}",
                specialized_id, dest_reg
            )
        });

        let stack_offset = self.slot_offset(specialized_id, "value")?;
        let _ = dest_reg;
        unsafe extern "C" {
            fn jit_rebox_array_float(slot: *mut JitVecSlot) -> u8;
        }
        self.emit_slot_addr(0, stack_offset);
        self.emit_call(jit_rebox_array_float as *const ());
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    /// Specialized push for Vec<LustInt> with inline fast path
    fn compile_vec_int_push(
        &mut self,
        vec_id: usize,
        value_reg: u8,
        is_last_push: bool,
    ) -> Result<()> {
        jit::log(|| {
            format!(
                "⚡ JIT: Specialized int push to vec #{} from reg {}",
                vec_id, value_reg
            )
        });

        let stack_offset = self.slot_offset(vec_id, "vec")?;

        unsafe extern "C" {
            fn jit_vec_int_push(
                vec_ptr: *mut *mut LustInt,
                vec_len: *mut usize,
                vec_cap: *mut usize,
                value: LustInt,
            ) -> u8;
        }

        if self.cached_specialized_vec != Some(vec_id) {
            self.emit_slot_addr(0, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x14, [x0, 0]
                ; ldr x15, [x0, 8]
                ; ldr x16, [x0, 16]
            );
            self.cached_specialized_vec = Some(vec_id);
            self.cached_select_imms = None;
        }

        let slow = self.ops.new_dynamic_label();
        let done = self.ops.new_dynamic_label();

        let val_num = if let Some(pin) = self.active_pin(value_reg) {
            pin.reg
        } else if self.hot_x0_in == Some(value_reg) {
            0
        } else {
            self.load_payload(3, value_reg);
            3
        };

        dynasm!(self.ops
            ; .arch aarch64
            ; cmp x15, x16
            ; b.hs => slow
            ; str X(val_num), [x14, x15, lsl #3]
            ; add x15, x15, 1
        );
        if is_last_push {
            self.emit_slot_addr(10, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; str x15, [x10, 8]
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; b => done
            ; => slow
        );
        self.emit_slot_addr(10, stack_offset);
        dynasm!(self.ops
            ; .arch aarch64
            ; str x15, [x10, 8]
            ; mov x3, X(val_num)
            ; mov x0, x10
            ; add x1, x10, 8
            ; add x2, x10, 16
        );
        self.emit_call(jit_vec_int_push as *const ());
        self.emit_fail_if_w0_zero();
        self.emit_slot_addr(0, stack_offset);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x14, [x0, 0]
            ; ldr x15, [x0, 8]
            ; ldr x16, [x0, 16]
            ; => done
        );
        self.cached_specialized_vec = Some(vec_id);

        Ok(())
    }

    /// Specialized push for Vec<u8> (booleans) with inline fast path
    fn compile_vec_bool_push(
        &mut self,
        vec_id: usize,
        value_reg: u8,
        is_last_push: bool,
    ) -> Result<()> {
        jit::log(|| {
            format!(
                "⚡ JIT: Specialized bool push to vec #{} from reg {}",
                vec_id, value_reg
            )
        });

        let stack_offset = self.slot_offset(vec_id, "vec")?;

        unsafe extern "C" {
            fn jit_vec_bool_push(
                vec_ptr: *mut *mut u8,
                vec_len: *mut usize,
                vec_cap: *mut usize,
                value: u8,
            ) -> u8;
        }

        if self.cached_specialized_vec != Some(vec_id) {
            self.emit_slot_addr(0, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x14, [x0, 0]
                ; ldr x15, [x0, 8]
                ; ldr x16, [x0, 16]
            );
            self.cached_specialized_vec = Some(vec_id);
            self.cached_select_imms = None;
        }

        let slow = self.ops.new_dynamic_label();
        let done = self.ops.new_dynamic_label();

        let (val_num, is_zero) = if let Some(pin) = self.active_pin(value_reg) {
            self.cached_bool_const = None;
            (pin.reg, false)
        } else if let Some(const_val) = self.const_registers.get(&value_reg) {
            match const_val {
                Value::Bool(false) => (0, true),
                Value::Bool(true) => {
                    if self.cached_bool_const != Some(true) {
                        dynasm!(self.ops ; .arch aarch64 ; movz w3, 1);
                        self.cached_bool_const = Some(true);
                    }
                    (3, false)
                }
                _ => {
                    self.load_bool_payload(3, value_reg);
                    self.cached_bool_const = None;
                    (3, false)
                }
            }
        } else if self.hot_x0_in == Some(value_reg) {
            self.cached_bool_const = None;
            (0, false)
        } else {
            self.load_bool_payload(3, value_reg);
            self.cached_bool_const = None;
            (3, false)
        };

        if is_zero {
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp x15, x16
                ; b.hs => slow
                ; strb wzr, [x14, x15]
                ; add x15, x15, 1
            );
        } else {
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp x15, x16
                ; b.hs => slow
                ; strb W(val_num), [x14, x15]
                ; add x15, x15, 1
            );
        }
        if is_last_push {
            self.emit_slot_addr(10, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; str x15, [x10, 8]
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; b => done
            ; => slow
        );
        self.cached_bool_const = None;
        self.emit_slot_addr(10, stack_offset);
        if is_zero {
            dynasm!(self.ops
                ; .arch aarch64
                ; str x15, [x10, 8]
                ; mov w3, wzr
                ; mov x0, x10
                ; add x1, x10, 8
                ; add x2, x10, 16
            );
        } else {
            dynasm!(self.ops
                ; .arch aarch64
                ; str x15, [x10, 8]
                ; mov w3, W(val_num)
                ; mov x0, x10
                ; add x1, x10, 8
                ; add x2, x10, 16
            );
        }
        self.emit_call(jit_vec_bool_push as *const ());
        self.emit_fail_if_w0_zero();
        self.emit_slot_addr(0, stack_offset);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x14, [x0, 0]
            ; ldr x15, [x0, 8]
            ; ldr x16, [x0, 16]
            ; => done
        );
        self.cached_specialized_vec = Some(vec_id);

        Ok(())
    }

    /// Specialized push for Vec<LustFloat> with inline fast path
    fn compile_vec_float_push(&mut self, vec_id: usize, value_reg: u8) -> Result<()> {
        jit::log(|| {
            format!(
                "⚡ JIT: Specialized float push to vec #{} from reg {}",
                vec_id, value_reg
            )
        });

        let stack_offset = self.slot_offset(vec_id, "vec")?;

        unsafe extern "C" {
            fn jit_vec_float_push(
                vec_ptr: *mut *mut LustFloat,
                vec_len: *mut usize,
                vec_cap: *mut usize,
                value: LustFloat,
            ) -> u8;
        }

        let slow = self.ops.new_dynamic_label();
        let done = self.ops.new_dynamic_label();

        self.emit_slot_addr(0, stack_offset);
        self.load_payload(3, value_reg);

        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x1, [x0, 8]
            ; ldr x2, [x0, 16]
            ; cmp x1, x2
            ; b.hs => slow
            ; ldr x10, [x0, 0]
            ; str x3, [x10, x1, lsl #3]
            ; add x1, x1, 1
            ; str x1, [x0, 8]
            ; b => done
            ; => slow
            ; add x1, x0, 8
            ; add x2, x0, 16
        );
        self.emit_call(jit_vec_float_push as *const ());
        self.emit_fail_if_w0_zero();
        dynasm!(self.ops
            ; .arch aarch64
            ; => done
        );

        Ok(())
    }

    /// Specialized element store (VecSet)
    fn compile_vec_set(&mut self, vec_id: usize, index_reg: u8, value_reg: u8) -> Result<()> {
        let specialized = self.specialized_values.get(&vec_id).cloned().ok_or_else(|| {
            crate::LustError::RuntimeError {
                message: format!("Specialized vec #{} not found", vec_id),
            }
        })?;
        let stack_offset = specialized.stack_offset;

        if self.cached_specialized_vec != Some(vec_id) {
            self.emit_slot_addr(0, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x14, [x0, 0]
                ; ldr x15, [x0, 8]
                ; ldr x16, [x0, 16]
            );
            self.cached_specialized_vec = Some(vec_id);
            self.cached_select_imms = None;
        }

        let idx_reg = self.operand_x(index_reg, 12);

        if !self.specialized_bounded_indices.contains(&(vec_id, index_reg)) {
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp X(idx_reg), x15
                ; b.hs >fail
            );
        }

        match specialized.kind {
            VecKind::Bool => {
                if let Some(pin) = self.active_pin(value_reg) {
                    dynasm!(self.ops ; .arch aarch64 ; strb W(pin.reg), [x14, X(idx_reg)]);
                } else if let Some(const_val) = self.const_registers.get(&value_reg) {
                    match const_val {
                        Value::Bool(false) => {
                            dynasm!(self.ops ; .arch aarch64 ; strb wzr, [x14, X(idx_reg)]);
                        }
                        Value::Bool(true) => {
                            dynasm!(self.ops
                                ; .arch aarch64
                                ; movz w0, 1
                                ; strb w0, [x14, X(idx_reg)]
                            );
                        }
                        _ => {
                            self.load_bool_payload(0, value_reg);
                            dynasm!(self.ops ; .arch aarch64 ; strb w0, [x14, X(idx_reg)]);
                        }
                    }
                } else {
                    self.load_bool_payload(0, value_reg);
                    dynasm!(self.ops ; .arch aarch64 ; strb w0, [x14, X(idx_reg)]);
                }
            }
            VecKind::Int => {
                if let Some(pin) = self.active_pin(value_reg) {
                    dynasm!(self.ops ; .arch aarch64 ; str X(pin.reg), [x14, X(idx_reg), lsl #3]);
                } else if let Some(const_val) = self.const_registers.get(&value_reg) {
                    if let Value::Int(i) = const_val {
                        self.emit_mov_imm64(0, *i as u64);
                    } else {
                        self.load_payload(0, value_reg);
                    }
                    dynasm!(self.ops ; .arch aarch64 ; str x0, [x14, X(idx_reg), lsl #3]);
                } else {
                    self.load_payload(0, value_reg);
                    dynasm!(self.ops ; .arch aarch64 ; str x0, [x14, X(idx_reg), lsl #3]);
                }
            }
            VecKind::Float => {
                self.load_payload(0, value_reg);
                dynasm!(self.ops ; .arch aarch64 ; str x0, [x14, X(idx_reg), lsl #3]);
            }
        }
        Ok(())
    }

    /// Specialized element load (VecGet)
    fn compile_vec_get(&mut self, dest_reg: u8, vec_id: usize, index_reg: u8) -> Result<()> {
        let specialized = self.specialized_values.get(&vec_id).cloned().ok_or_else(|| {
            crate::LustError::RuntimeError {
                message: format!("Specialized vec #{} not found", vec_id),
            }
        })?;
        let stack_offset = specialized.stack_offset;

        if self.cached_specialized_vec != Some(vec_id) {
            self.emit_slot_addr(0, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x14, [x0, 0]
                ; ldr x15, [x0, 8]
                ; ldr x16, [x0, 16]
            );
            self.cached_specialized_vec = Some(vec_id);
            self.cached_select_imms = None;
        }

        let idx_reg = self.operand_x(index_reg, 12);

        if !self.specialized_bounded_indices.contains(&(vec_id, index_reg)) {
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp X(idx_reg), x15
                ; b.hs >fail
            );
        }

        match specialized.kind {
            VecKind::Bool => {
                if let Some(pin) = self.active_pin(dest_reg) {
                    if pin.ty == ValueType::Bool {
                        self.ensure_pin_tag(dest_reg, pin);
                        self.mark_dirty(dest_reg);
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; ldrb W(pin.reg), [x14, X(idx_reg)]
                        );
                        return Ok(());
                    }
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldrb w0, [x14, X(idx_reg)]
                );
                self.store_from_x0(dest_reg, ValueTag::Bool.as_u8());
            }
            VecKind::Int => {
                if let Some(pin) = self.active_pin(dest_reg) {
                    if pin.ty == ValueType::Int {
                        self.ensure_pin_tag(dest_reg, pin);
                        self.mark_dirty(dest_reg);
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; ldr X(pin.reg), [x14, X(idx_reg), lsl #3]
                        );
                        return Ok(());
                    }
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x0, [x14, X(idx_reg), lsl #3]
                );
                self.store_from_x0(dest_reg, ValueTag::Int.as_u8());
            }
            VecKind::Float => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr d0, [x14, X(idx_reg), lsl #3]
                );
                self.store_d0_as_float(dest_reg);
            }
        }
        Ok(())
    }

    /// Specialized element load with bounds check (VecIndexOk)
    fn compile_vec_index_ok(
        &mut self,
        value_dest: u8,
        condition_dest: u8,
        vec_id: usize,
        index_reg: u8,
    ) -> Result<()> {
        let specialized = self.specialized_values.get(&vec_id).cloned().ok_or_else(|| {
            crate::LustError::RuntimeError {
                message: format!("Specialized vec #{} not found", vec_id),
            }
        })?;
        let stack_offset = specialized.stack_offset;

        if self.cached_specialized_vec != Some(vec_id) {
            self.emit_slot_addr(0, stack_offset);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x14, [x0, 0]
                ; ldr x15, [x0, 8]
                ; ldr x16, [x0, 16]
            );
            self.cached_specialized_vec = Some(vec_id);
            self.cached_select_imms = None;
        }

        let is_bounded = self.specialized_bounded_indices.contains(&(vec_id, index_reg));
        let idx_reg = self.operand_x(index_reg, 12);
        let out_of_range = self.ops.new_dynamic_label();
        let done = self.ops.new_dynamic_label();

        if !is_bounded {
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp X(idx_reg), x15
                ; b.hs => out_of_range
            );
        }

        match specialized.kind {
            VecKind::Bool => {
                if let Some(pin) = self.active_pin(value_dest) {
                    if pin.ty == ValueType::Bool {
                        self.ensure_pin_tag(value_dest, pin);
                        self.mark_dirty(value_dest);
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; ldrb W(pin.reg), [x14, X(idx_reg)]
                        );
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; ldrb w0, [x14, X(idx_reg)]);
                        self.store_from_x0(value_dest, ValueTag::Bool.as_u8());
                    }
                } else {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldrb w0, [x14, X(idx_reg)]
                    );
                    self.store_from_x0(value_dest, ValueTag::Bool.as_u8());
                }
            }
            VecKind::Int => {
                if let Some(pin) = self.active_pin(value_dest) {
                    if pin.ty == ValueType::Int {
                        self.ensure_pin_tag(value_dest, pin);
                        self.mark_dirty(value_dest);
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; ldr X(pin.reg), [x14, X(idx_reg), lsl #3]
                        );
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x14, X(idx_reg), lsl #3]);
                        self.store_from_x0(value_dest, ValueTag::Int.as_u8());
                    }
                } else {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x0, [x14, X(idx_reg), lsl #3]
                    );
                    self.store_from_x0(value_dest, ValueTag::Int.as_u8());
                }
            }
            VecKind::Float => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr d0, [x14, X(idx_reg), lsl #3]
                );
                self.store_d0_as_float(value_dest);
            }
        }
        if let Some(pin) = self.active_pin(condition_dest) {
            if pin.ty == ValueType::Bool {
                self.ensure_pin_tag(condition_dest, pin);
                self.mark_dirty(condition_dest);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; movz W(pin.reg), 1
                );
            } else {
                dynasm!(self.ops ; .arch aarch64 ; movz x0, 1);
                self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
            }
        } else {
            dynasm!(self.ops
                ; .arch aarch64
                ; movz x0, 1
            );
            self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
        }
        if !is_bounded {
            dynasm!(self.ops
                ; .arch aarch64
                ; b => done
                ; => out_of_range
            );
            self.cached_specialized_vec = None;
            self.compile_load_const(value_dest, &Value::Nil)?;
            if let Some(pin) = self.active_pin(condition_dest) {
                if pin.ty == ValueType::Bool {
                    self.ensure_pin_tag(condition_dest, pin);
                    self.mark_dirty(condition_dest);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; mov W(pin.reg), wzr
                    );
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; mov x0, xzr);
                    self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
                }
            } else {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; mov x0, xzr
                );
                self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; => done
            );
        }
        Ok(())
    }

    /// Get length of specialized Vec
    fn compile_vec_int_len(&mut self, vec_id: usize, dest_reg: u8) -> Result<()> {
        jit::log(|| {
            format!(
                "⚡ JIT: Specialized len of vec #{} to reg {}",
                vec_id, dest_reg
            )
        });

        let stack_offset = self.slot_offset(vec_id, "vec")?;
        self.emit_slot_addr(11, stack_offset);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x0, [x11, 8]
        );
        self.store_from_x0(dest_reg, ValueTag::Int.as_u8());

        Ok(())
    }

    /// Drop a specialized value without reboxing (cleanup for leaked specializations)
    pub(super) fn compile_drop_specialized(
        &mut self,
        specialized_id: usize,
        layout: &SpecializedLayout,
    ) -> Result<()> {
        match vec_kind(layout) {
            Some(VecKind::Int) => self.compile_drop_vec_int(specialized_id),
            Some(VecKind::Bool) => self.compile_drop_vec_bool(specialized_id),
            Some(VecKind::Float) => self.compile_drop_vec_float(specialized_id),
            None => Err(crate::LustError::RuntimeError {
                message: format!("Drop not yet implemented for layout {:?}", layout),
            }),
        }
    }

    /// Drop Vec<LustInt> without reboxing
    fn compile_drop_vec_int(&mut self, vec_id: usize) -> Result<()> {
        jit::log(|| format!("🗑️  JIT: Dropping specialized int vec #{}", vec_id));

        let stack_offset = self.slot_offset(vec_id, "vec")?;

        unsafe extern "C" {
            fn jit_drop_vec_int(slot: *mut JitVecSlot);
        }
        self.emit_slot_addr(0, stack_offset);
        self.emit_call(jit_drop_vec_int as *const ());

        Ok(())
    }

    /// Drop Vec<u8> (booleans) without reboxing
    fn compile_drop_vec_bool(&mut self, vec_id: usize) -> Result<()> {
        jit::log(|| format!("🗑️  JIT: Dropping specialized bool vec #{}", vec_id));

        let stack_offset = self.slot_offset(vec_id, "vec")?;

        unsafe extern "C" {
            fn jit_drop_vec_bool(slot: *mut JitVecSlot);
        }
        self.emit_slot_addr(0, stack_offset);
        self.emit_call(jit_drop_vec_bool as *const ());

        Ok(())
    }

    /// Drop Vec<LustFloat> without reboxing
    fn compile_drop_vec_float(&mut self, vec_id: usize) -> Result<()> {
        jit::log(|| format!("🗑️  JIT: Dropping specialized float vec #{}", vec_id));

        let stack_offset = self.slot_offset(vec_id, "vec")?;

        unsafe extern "C" {
            fn jit_drop_vec_float(slot: *mut JitVecSlot);
        }
        self.emit_slot_addr(0, stack_offset);
        self.emit_call(jit_drop_vec_float as *const ());

        Ok(())
    }

    /// Allocate stack space for specialized values.
    /// Returns the offset from x29 (negative value).
    fn allocate_specialized_stack(&mut self, _size: usize, _align: usize) -> i32 {
        // Slots grow downward from SPECIALIZED_BASE_OFFSET below the frame
        // pointer; each is SPECIALIZED_SLOT_SIZE bytes (24 used + padding).
        let allocation_offset = self.specialized_values.len() as i32 * SPECIALIZED_SLOT_SIZE;
        SPECIALIZED_BASE_OFFSET - allocation_offset
    }
}
