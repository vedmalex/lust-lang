use super::*;
impl JitCompiler {
    pub(super) fn compile_load_const(&mut self, dest: u8, value: &Value) -> Result<()> {
        if (self.scalar_regfile || (self.function_mode && !self.function_frame.1))
            && matches!(value, Value::Function(_) | Value::Nil)
        {
            // The direct-call ABI does not read the callee register, and a
            // Nil constant owns nothing. Leaving the slot untouched keeps
            // the regfile from materializing a `Value`.
            return Ok(());
        }
        match value {
            Value::Int(i) => {
                if let Some(pin) = self.active_pin(dest) {
                    if pin.class == pins::PinClass::Carried && pin.ty == ValueType::Int {
                        self.ensure_pin_tag(dest, pin);
                        self.mark_dirty(dest);
                        self.emit_mov_imm64(pin.reg, *i as u64);
                        return Ok(());
                    }
                }
                self.emit_mov_imm64(0, *i as u64);
                self.store_from_x0(dest, ValueTag::Int.as_u8());
                Ok(())
            }

            Value::Float(f) => {
                self.emit_mov_imm64(0, f.to_bits());
                self.hot_d0_in = None;
                dynasm!(self.ops ; .arch aarch64 ; fmov d0, x0);
                self.store_d0_as_float(dest);
                Ok(())
            }

            Value::Bool(b) => {
                if let Some(pin) = self.active_pin(dest) {
                    if pin.class == pins::PinClass::Carried && pin.ty == ValueType::Bool {
                        self.ensure_pin_tag(dest, pin);
                        self.mark_dirty(dest);
                        if *b {
                            dynasm!(self.ops ; .arch aarch64 ; movz W(pin.reg), 1);
                        } else {
                            dynasm!(self.ops ; .arch aarch64 ; mov X(pin.reg), xzr);
                        }
                        return Ok(());
                    }
                }
                let bool_val = u32::from(*b);
                dynasm!(self.ops ; .arch aarch64 ; movz x0, #bool_val);
                self.store_from_x0(dest, ValueTag::Bool.as_u8());
                Ok(())
            }

            // Plain values that own nothing: tag + payload, with the old
            // value dropped only if it owned something.
            Value::Nil => self.store_plain_constant(dest, 0, value),
            Value::Function(idx) => self.store_plain_constant(dest, *idx as u64, value),

            _ => self.copy_owned_constant(dest, value),
        }
    }

    fn store_plain_constant(&mut self, dest: u8, payload: u64, value: &Value) -> Result<()> {
        let scalar_max_tag = ValueTag::Float.as_u8() as u32;
        // The discriminant byte of `#[repr(C, u8)] Value` (`ValueTag` is a
        // coarser classification and numbers `Function` differently).
        // SAFETY: reading the first byte of a live `Value`.
        let tag = unsafe { *(value as *const Value as *const u8) } as u32;
        // A destination known to hold nothing owned is simply overwritten.
        if self.scalar_registers.contains_key(&dest)
            || (self.function_mode && !self.function_frame.1)
        {
            self.emit_mov_imm64(0, payload);
            self.store_tag_imm(dest, tag as u8);
            self.store_payload(dest, 0);
            return Ok(());
        }
        let done = self.ops.new_dynamic_label();
        self.load_tag_from_memory(9, dest);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w9, #scalar_max_tag
            ; b.ls >plain
        );
        // A function index held from the previous iteration of a loop that
        // reloads it is as plain as a scalar.
        if matches!(value, Value::Function(_)) {
            dynasm!(self.ops ; .arch aarch64 ; cmp w9, #tag ; b.ne >owned);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; b >owned);
        }
        dynasm!(self.ops ; .arch aarch64 ; plain:);
        self.emit_mov_imm64(0, payload);
        self.store_tag_imm(dest, tag as u8);
        self.store_payload(dest, 0);
        dynasm!(self.ops
            ; .arch aarch64
            ; b => done
            ; owned:
        );
        self.copy_owned_constant(dest, value)?;
        dynasm!(self.ops ; .arch aarch64 ; => done);
        Ok(())
    }

    fn copy_owned_constant(&mut self, dest: u8, value: &Value) -> Result<()> {
        let src_ptr = self.retain_value(value.clone());
        unsafe extern "C" {
            fn jit_move_safe(src_ptr: *const Value, dest_ptr: *mut Value) -> u8;
        }
        self.emit_mov_imm64(0, src_ptr as usize as u64);
        self.emit_reg_addr(1, dest);
        self.emit_call(jit_move_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }

    pub(super) fn compile_move(&mut self, dest: u8, src: u8) -> Result<()> {
        // A pinned destination is only ever written with a scalar of its
        // own type (see pins::plan), so the move is a register copy; the
        // helper below would update memory and leave the pin stale.
        if let Some(pin) = self.pin_for_write(dest) {
            match pin.ty {
                ValueType::Float => {
                    self.load_payload_f(0, src);
                    self.store_d0_as_float(dest);
                }
                ValueType::Int => {
                    self.load_payload(0, src);
                    self.store_from_x0(dest, ValueTag::Int.as_u8());
                }
                _ => {
                    self.load_payload(0, src);
                    self.store_from_x0(dest, ValueTag::Bool.as_u8());
                }
            }
            return Ok(());
        }
        // A source of known scalar type is a payload copy plus a typed
        // store (which drops whatever the destination held).
        match self.scalar_registers.get(&src).copied() {
            Some(ValueType::Float) => {
                self.load_payload_f(0, src);
                self.store_d0_as_float(dest);
                return Ok(());
            }
            Some(ValueType::Int) => {
                self.load_payload(0, src);
                self.store_from_x0(dest, ValueTag::Int.as_u8());
                return Ok(());
            }
            Some(ValueType::Bool) => {
                self.load_bool_payload(0, src);
                self.store_from_x0(dest, ValueTag::Bool.as_u8());
                return Ok(());
            }
            _ => {}
        }
        if self.payload_slots {
            self.load_payload(0, src);
            self.store_payload(dest, 0);
            return Ok(());
        }
        self.emit_reg_addr(11, src);
        self.emit_clone_x11_into(dest);
        Ok(())
    }

    pub(super) fn compile_get_index(&mut self, dest: u8, array: u8, index: u8) -> Result<()> {
        let index_is_int = self.scalar_registers.get(&index) == Some(&ValueType::Int)
            || self.pins.get(&index).is_some_and(|p| p.ty == ValueType::Int);

        if let Some(hoisted) = self.hoisted_array
            && hoisted.array_reg == array
            && index_is_int
        {
            let idx_reg = self.operand_x(index, 12);
            let bounds_check_needed = hoisted.bounded_index != Some(index);
            if bounds_check_needed {
                if let Some(len_r) = hoisted.len_reg {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; cmp X(idx_reg), X(len_r)
                        ; b.hs >fail
                    );
                }
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; add x10, X(hoisted.ptr_reg), X(idx_reg), lsl #4
            );
            self.emit_reg_addr(11, dest);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldp x0, x1, [x10]
                ; stp x0, x1, [x11]
            );
            if let Some(pin) = self.pin_for_write(dest) {
                match pin.ty {
                    ValueType::Int => {
                        dynasm!(self.ops ; .arch aarch64 ; mov X(pin.reg), x1);
                    }
                    ValueType::Bool => {
                        dynasm!(self.ops ; .arch aarch64 ; uxtb W(pin.reg), w1);
                    }
                    ValueType::Float => {
                        dynasm!(self.ops ; .arch aarch64 ; fmov D(pin.reg), x1);
                    }
                    _ => {}
                }
            }
            return Ok(());
        }

        let array_tag = ValueTag::Array.as_u8() as u32;
        let int_tag = ValueTag::Int.as_u8() as u32;
        unsafe extern "C" {
            fn jit_array_get_safe(
                vm_ptr: *mut crate::VM,
                array_value: *const Value,
                index_value: *const Value,
                out: *mut Value,
            ) -> u8;
        }

        self.load_tag(0, array);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w0, #array_tag
            ; b.ne >fail
        );
        self.load_tag(0, index);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w0, #int_tag
            ; b.ne >fail
            ; mov x0, x20
        );
        self.emit_reg_addr(1, array);
        self.emit_reg_addr(2, index);
        self.emit_reg_addr(3, dest);
        self.emit_call(jit_array_get_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }

    pub(super) fn compile_try_get_index(&mut self, dest: u8, array: u8, index: u8) -> Result<()> {
        unsafe extern "C" {
            fn jit_array_index_result_safe(
                vm_ptr: *mut crate::VM,
                array_value: *const Value,
                index_value: *const Value,
                out: *mut Value,
            ) -> u8;
        }

        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, array);
        self.emit_reg_addr(2, index);
        self.emit_reg_addr(3, dest);
        self.emit_call(jit_array_index_result_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }

    /// `array[index] = value`. Inline when the index is a known int: the
    /// array tag, the bounds, and that neither the old element nor the
    /// value owns anything are checked, then the value's two words replace
    /// the element. Anything else goes through the helper, which fails to
    /// the interpreter for an error.
    pub(super) fn compile_set_index(
        &mut self,
        array: u8,
        index: u8,
        value: u8,
        element_type: Option<ValueType>,
    ) -> Result<()> {
        let index_is_int = self.scalar_registers.get(&index) == Some(&ValueType::Int)
            || self.pins.get(&index).is_some_and(|p| p.ty == ValueType::Int);

        if let Some(hoisted) = self.hoisted_array
            && hoisted.array_reg == array
            && index_is_int
        {
            let idx_reg = self.operand_x(index, 12);
            let bounds_check_needed = hoisted.bounded_index != Some(index);
            if bounds_check_needed {
                if let Some(len_r) = hoisted.len_reg {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; cmp X(idx_reg), X(len_r)
                        ; b.hs >fail
                    );
                }
            }
            let val_ty = self
                .scalar_registers
                .get(&value)
                .copied()
                .or_else(|| self.pins.get(&value).map(|p| p.ty))
                .or(element_type);
            let scalar_slot = val_ty
                .is_some_and(|ty| matches!(ty, ValueType::Int | ValueType::Float | ValueType::Bool));

            dynasm!(self.ops
                ; .arch aarch64
                ; add x10, X(hoisted.ptr_reg), X(idx_reg), lsl #4
            );

            if !scalar_slot {
                let scalar_max_tag = ValueTag::Float.as_u8() as u32;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldrb w9, [x10]
                    ; cmp w9, #scalar_max_tag
                    ; b.hi >fail
                );
            }

            if let Some(ty) = val_ty {
                let tag = match ty {
                    ValueType::Int => ValueTag::Int.as_u8(),
                    ValueType::Float => ValueTag::Float.as_u8(),
                    _ => ValueTag::Bool.as_u8(),
                } as u32;
                let same_element_tag = element_type == Some(ty);
                if same_element_tag {
                    if let Some(pin) = self.active_pin(value) {
                        dynasm!(self.ops ; .arch aarch64 ; str X(pin.reg), [x10, #8]);
                    } else if let Some(const_val) = self.const_registers.get(&value) {
                        match const_val {
                            Value::Bool(false) => {
                                dynasm!(self.ops ; .arch aarch64 ; str xzr, [x10, #8]);
                            }
                            Value::Bool(true) => {
                                dynasm!(self.ops ; .arch aarch64 ; movz x1, 1 ; str x1, [x10, #8]);
                            }
                            Value::Int(0) => {
                                dynasm!(self.ops ; .arch aarch64 ; str xzr, [x10, #8]);
                            }
                            Value::Int(i) => {
                                self.emit_mov_imm64(1, *i as u64);
                                dynasm!(self.ops ; .arch aarch64 ; str x1, [x10, #8]);
                            }
                            Value::Float(f) if f.to_bits() == 0 => {
                                dynasm!(self.ops ; .arch aarch64 ; str xzr, [x10, #8]);
                            }
                            Value::Float(f) => {
                                self.emit_mov_imm64(1, f.to_bits());
                                dynasm!(self.ops ; .arch aarch64 ; str x1, [x10, #8]);
                            }
                            _ => {
                                self.load_payload(1, value);
                                dynasm!(self.ops ; .arch aarch64 ; str x1, [x10, #8]);
                            }
                        }
                    } else if self.hot_x0_in == Some(value) {
                        dynasm!(self.ops ; .arch aarch64 ; str x0, [x10, #8]);
                    } else if ty == ValueType::Bool {
                        self.load_bool_payload(1, value);
                        dynasm!(self.ops ; .arch aarch64 ; str x1, [x10, #8]);
                    } else {
                        self.load_payload(1, value);
                        dynasm!(self.ops ; .arch aarch64 ; str x1, [x10, #8]);
                    }
                } else {
                    if let Some(pin) = self.active_pin(value) {
                        dynasm!(self.ops ; .arch aarch64 ; mov X(1), X(pin.reg));
                    } else if let Some(const_val) = self.const_registers.get(&value) {
                        match const_val {
                            Value::Bool(false) => {
                                dynasm!(self.ops ; .arch aarch64 ; mov x1, xzr);
                            }
                            Value::Bool(true) => {
                                dynasm!(self.ops ; .arch aarch64 ; movz x1, 1);
                            }
                            Value::Int(i) => {
                                self.emit_mov_imm64(1, *i as u64);
                            }
                            Value::Float(f) => {
                                self.emit_mov_imm64(1, f.to_bits());
                            }
                            _ => self.load_payload(1, value),
                        }
                    } else if self.hot_x0_in == Some(value) {
                        dynasm!(self.ops ; .arch aarch64 ; mov x1, x0);
                    } else if ty == ValueType::Bool {
                        self.load_bool_payload(1, value);
                    } else {
                        self.load_payload(1, value);
                    }
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; movz x0, #tag
                        ; stp x0, x1, [x10]
                    );
                }
            } else {
                self.load_tag_from_memory(13, value);
                let scalar_max_tag = ValueTag::Float.as_u8() as u32;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w13, #scalar_max_tag
                    ; b.hi >fail
                );
                self.emit_reg_addr(11, value);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldp x0, x1, [x11]
                    ; stp x0, x1, [x10]
                );
            }
            return Ok(());
        }

        unsafe extern "C" {
            fn jit_array_set_index_safe(
                array_value: *const Value,
                index_value: *const Value,
                value: *const Value,
            ) -> u8;
        }

        if let (Some(layout), true) = (
            jit::layout::rc_vec_layout(),
            index_is_int,
        ) {
            let array_tag = ValueTag::Array.as_u8() as u32;
            let scalar_max_tag = ValueTag::Float.as_u8() as u32;
            let rc_offset = layout.array_rc_offset as u32;
            let len_offset = layout.len_offset as u32;
            let ptr_offset = layout.ptr_offset as u32;
            let _value_size = mem::size_of::<Value>() as u32;
            let done = self.ops.new_dynamic_label();
            let slow = self.ops.new_dynamic_label();
            let is_hoisted = self.hoisted_array.as_ref().is_some_and(|h| h.array_reg == array);
            if !is_hoisted && !self.array_registers.contains(&array) {
                self.load_tag(0, array);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w0, #array_tag
                    ; b.ne => slow
                );
            }
            if let Some(pin) = self.active_pin(index) {
                dynasm!(self.ops ; .arch aarch64 ; mov X(12), X(pin.reg));
            } else {
                self.load_payload(12, index);
            }
            // A monomorphic scalar array never holds an owned element, so
            // the old tag (and its rc_dec) can be skipped.
            let scalar_slot = element_type
                .is_some_and(|ty| matches!(ty, ValueType::Int | ValueType::Float | ValueType::Bool))
                && self.scalar_registers.get(&value) == element_type.as_ref();
            if let Some(hoisted) = self.hoisted_array && hoisted.array_reg == array {
                if let Some(len_r) = hoisted.len_reg {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; cmp x12, X(len_r)
                        ; b.hs >fail
                        ; add x10, X(hoisted.ptr_reg), x12, lsl #4
                    );
                } else {
                    self.emit_reg_addr(11, array);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x9, [x11, #rc_offset]
                        ; ldr x10, [x9, #len_offset]
                        ; cmp x12, x10
                        ; b.hs >fail
                        ; add x10, X(hoisted.ptr_reg), x12, lsl #4
                    );
                }
            } else {
                self.emit_reg_addr(11, array);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x9, [x11, #rc_offset]
                    ; ldr x10, [x9, #len_offset]
                    ; cmp x12, x10
                    ; b.hs >fail
                    ; ldr x10, [x9, #ptr_offset]
                    ; add x10, x10, x12, lsl #4
                );
            }
            if !scalar_slot {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldrb w9, [x10]
                    ; cmp w9, #scalar_max_tag
                    ; b.hi => slow
                );
            }
            if let Some(ty) = self.scalar_registers.get(&value).copied() {
                let tag = match ty {
                    ValueType::Int => ValueTag::Int.as_u8(),
                    ValueType::Float => ValueTag::Float.as_u8(),
                    _ => ValueTag::Bool.as_u8(),
                } as u32;
                if let Some(pin) = self.active_pin(value) {
                    dynasm!(self.ops ; .arch aarch64 ; mov X(1), X(pin.reg));
                } else if let Some(const_val) = self.const_registers.get(&value) {
                    match const_val {
                        Value::Bool(false) => {
                            dynasm!(self.ops ; .arch aarch64 ; mov x1, xzr);
                        }
                        Value::Bool(true) => {
                            dynasm!(self.ops ; .arch aarch64 ; movz x1, 1);
                        }
                        Value::Int(i) => {
                            self.emit_mov_imm64(1, *i as u64);
                        }
                        Value::Float(f) => {
                            self.emit_mov_imm64(1, f.to_bits());
                        }
                        _ => self.load_payload(1, value),
                    }
                } else if self.hot_x0_in == Some(value) {
                    dynasm!(self.ops ; .arch aarch64 ; mov x1, x0);
                } else if ty == ValueType::Bool {
                    self.load_bool_payload(1, value);
                } else {
                    self.load_payload(1, value);
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; movz x0, #tag
                    ; stp x0, x1, [x10]
                    ; b => done
                );
            } else {
                self.load_tag_from_memory(13, value);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w13, #scalar_max_tag
                    ; b.hi => slow
                );
                self.emit_reg_addr(11, value);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldp x0, x1, [x11]
                    ; stp x0, x1, [x10]
                    ; b => done
                );
            }
            dynasm!(self.ops ; .arch aarch64 ; => slow);
            self.emit_reg_addr(0, array);
            self.emit_reg_addr(1, index);
            self.emit_reg_addr(2, value);
            self.emit_call(jit_array_set_index_safe as *const ());
            self.emit_fail_if_w0_zero();
            dynasm!(self.ops ; .arch aarch64 ; => done);
            return Ok(());
        }

        self.emit_reg_addr(0, array);
        self.emit_reg_addr(1, index);
        self.emit_reg_addr(2, value);
        self.emit_call(jit_array_set_index_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }

    pub(super) fn compile_array_index_ok(
        &mut self,
        value_dest: u8,
        condition_dest: u8,
        array: u8,
        index: u8,
        value_type: Option<ValueType>,
    ) -> Result<()> {
        let index_is_int = self.scalar_registers.get(&index) == Some(&ValueType::Int)
            || self.pins.get(&index).is_some_and(|p| p.ty == ValueType::Int);

        if let Some(hoisted) = self.hoisted_array
            && hoisted.array_reg == array
            && index_is_int
            && let Some(ty) = value_type
        {
            let idx_reg = self.operand_x(index, 12);
            if let Some(len_r) = hoisted.len_reg {
                let out_of_range = self.ops.new_dynamic_label();
                let done = self.ops.new_dynamic_label();
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp X(idx_reg), X(len_r)
                    ; b.hs => out_of_range
                    ; add x10, X(hoisted.ptr_reg), X(idx_reg), lsl #4
                    ; ldrb w9, [x10]
                );
                let expected_tag = match ty {
                    ValueType::Int => ValueTag::Int,
                    ValueType::Float => ValueTag::Float,
                    _ => ValueTag::Bool,
                }
                .as_u8() as u32;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w9, #expected_tag
                    ; b.ne >fail
                );
                match ty {
                    ValueType::Float => {
                        dynasm!(self.ops ; .arch aarch64 ; ldr d0, [x10, 8]);
                        self.store_d0_as_float(value_dest);
                    }
                    ValueType::Int => {
                        dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x10, 8]);
                        self.store_from_x0(value_dest, ValueTag::Int.as_u8());
                    }
                    _ => {
                        dynasm!(self.ops ; .arch aarch64 ; ldrb w0, [x10, 8]);
                        self.store_from_x0(value_dest, ValueTag::Bool.as_u8());
                    }
                }
                self.pending_scalar = Some((value_dest, ty));
                dynasm!(self.ops ; .arch aarch64 ; movz x0, 1);
                self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
                dynasm!(self.ops
                    ; .arch aarch64
                    ; b => done
                    ; => out_of_range
                );
                self.compile_load_const(value_dest, &Value::Nil)?;
                dynasm!(self.ops ; .arch aarch64 ; mov x0, xzr);
                self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
                dynasm!(self.ops
                    ; .arch aarch64
                    ; => done
                );
                return Ok(());
            }
        }

        unsafe extern "C" {
            fn jit_array_index_ok_safe(
                array_value: *const Value,
                index_value: *const Value,
                value_out: *mut Value,
                condition_out: *mut Value,
            ) -> u8;
        }

        if let (Some(layout), true) = (
            jit::layout::rc_vec_layout(),
            index_is_int,
        ) {
            // Inline: bounds from the Vec's length, a scalar element copied
            // as tag + payload. An element that owns something, or a
            // destination that does, goes through the helper.
            let array_tag = ValueTag::Array.as_u8() as u32;
            let scalar_max_tag = ValueTag::Float.as_u8() as u32;
            let rc_offset = layout.array_rc_offset as u32;
            let len_offset = layout.len_offset as u32;
            let ptr_offset = layout.ptr_offset as u32;
            let _value_size = mem::size_of::<Value>() as u32;
            let done = self.ops.new_dynamic_label();
            let out_of_range = self.ops.new_dynamic_label();
            let slow = self.ops.new_dynamic_label();
            let is_hoisted = self.hoisted_array.as_ref().is_some_and(|h| h.array_reg == array);
            if !is_hoisted && !self.array_registers.contains(&array) {
                self.load_tag(0, array);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w0, #array_tag
                    ; b.ne => slow
                );
                self.array_registers.insert(array);
            }
            if let Some(pin) = self.active_pin(index) {
                dynasm!(self.ops ; .arch aarch64 ; mov X(12), X(pin.reg));
            } else {
                self.load_payload(12, index);
            }
            if let Some(hoisted) = self.hoisted_array && hoisted.array_reg == array {
                if let Some(len_r) = hoisted.len_reg {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; cmp x12, X(len_r)
                        ; b.hs => out_of_range
                        ; add x10, X(hoisted.ptr_reg), x12, lsl #4
                        ; ldrb w9, [x10]
                    );
                } else {
                    self.emit_reg_addr(11, array);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x9, [x11, #rc_offset]
                        ; ldr x10, [x9, #len_offset]
                        ; cmp x12, x10
                        ; b.hs => out_of_range
                        ; add x10, X(hoisted.ptr_reg), x12, lsl #4
                        ; ldrb w9, [x10]
                    );
                }
            } else {
                self.emit_reg_addr(11, array);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x9, [x11, #rc_offset]
                    ; ldr x10, [x9, #len_offset]
                    ; cmp x12, x10
                    ; b.hs => out_of_range
                    ; ldr x10, [x9, #ptr_offset]
                    ; add x10, x10, x12, lsl #4
                    ; ldrb w9, [x10]
                );
            }
            if let Some(ty) = value_type {
                // An element of the expected scalar type: a typed store,
                // after which the destination is known to hold that type.
                // Any other element fails to the interpreter.
                let expected_tag = match ty {
                    ValueType::Int => ValueTag::Int,
                    ValueType::Float => ValueTag::Float,
                    _ => ValueTag::Bool,
                }
                .as_u8() as u32;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w9, #expected_tag
                    ; b.ne >fail
                );
                match ty {
                    ValueType::Float => {
                        dynasm!(self.ops ; .arch aarch64 ; ldr d0, [x10, 8]);
                        self.store_d0_as_float(value_dest);
                    }
                    ValueType::Int => {
                        dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x10, 8]);
                        self.store_from_x0(value_dest, ValueTag::Int.as_u8());
                        self.hot_x0 = Some(value_dest);
                    }
                    _ => {
                        dynasm!(self.ops ; .arch aarch64 ; ldrb w0, [x10, 8]);
                        self.store_from_x0(value_dest, ValueTag::Bool.as_u8());
                        self.hot_x0 = Some(value_dest);
                    }
                }
                self.pending_scalar = Some((value_dest, ty));
            } else {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w9, #scalar_max_tag
                    ; b.hi => slow
                );
                // The destination must hold nothing owned for a plain copy.
                self.load_tag_from_memory(13, value_dest);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w13, #scalar_max_tag
                    ; b.hi => slow
                    ; ldp x0, x1, [x10]
                );
                self.emit_reg_addr(11, value_dest);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; stp x0, x1, [x11]
                );
                if let Some(pin) = self.pin_for_write(value_dest) {
                    match pin.ty {
                        ValueType::Int => {
                            dynasm!(self.ops ; .arch aarch64 ; mov X(pin.reg), x1);
                        }
                        ValueType::Bool => {
                            dynasm!(self.ops ; .arch aarch64 ; uxtb W(pin.reg), w1);
                        }
                        ValueType::Float => {
                            dynasm!(self.ops ; .arch aarch64 ; fmov D(pin.reg), x1);
                        }
                        _ => {}
                    }
                }
            }
            dynasm!(self.ops ; .arch aarch64 ; movz x0, 1);
            self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
            dynasm!(self.ops
                ; .arch aarch64
                ; b => done
                ; => out_of_range
            );
            self.compile_load_const(value_dest, &Value::Nil)?;
            dynasm!(self.ops ; .arch aarch64 ; mov x0, xzr);
            self.store_from_x0(condition_dest, ValueTag::Bool.as_u8());
            dynasm!(self.ops
                ; .arch aarch64
                ; b => done
                ; => slow
            );
            self.emit_reg_addr(0, array);
            self.emit_reg_addr(1, index);
            self.emit_reg_addr(2, value_dest);
            self.emit_reg_addr(3, condition_dest);
            self.emit_call(jit_array_index_ok_safe as *const ());
            self.emit_fail_if_w0_zero();
            self.reload_pin_from_memory(value_dest);
            self.reload_pin_from_memory(condition_dest);
            dynasm!(self.ops ; .arch aarch64 ; => done);
            return Ok(());
        }

        self.emit_reg_addr(0, array);
        self.emit_reg_addr(1, index);
        self.emit_reg_addr(2, value_dest);
        self.emit_reg_addr(3, condition_dest);
        self.emit_call(jit_array_index_ok_safe as *const ());
        self.emit_fail_if_w0_zero();
        self.reload_pin_from_memory(value_dest);
        self.reload_pin_from_memory(condition_dest);
        Ok(())
    }

    pub(super) fn compile_array_index_ok_branch(
        &mut self,
        value_dest: u8,
        _condition_dest: u8,
        array: u8,
        index: u8,
        value_type: Option<ValueType>,
        expect_truthy: bool,
        label: usize,
        dead_condition: bool,
    ) -> Result<bool> {
        let index_is_int = self.scalar_registers.get(&index) == Some(&ValueType::Int)
            || self.pins.get(&index).is_some_and(|p| p.ty == ValueType::Int);

        if let Some(hoisted) = self.hoisted_array
            && hoisted.array_reg == array
            && index_is_int
            && let Some(ty) = value_type
            && let Some(len_r) = hoisted.len_reg
            && dead_condition
            && !expect_truthy
        {
            let target = self.function_label(label);
            self.emit_back_edge_gas_check(label);
            let idx_reg = self.operand_x(index, 12);
            let bounds_check_needed = hoisted.bounded_index != Some(index);
            if bounds_check_needed {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp X(idx_reg), X(len_r)
                    ; b.hs => target
                );
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; add x10, X(hoisted.ptr_reg), X(idx_reg), lsl #4
                ; ldrb w9, [x10]
            );
            let expected_tag = match ty {
                ValueType::Int => ValueTag::Int,
                ValueType::Float => ValueTag::Float,
                _ => ValueTag::Bool,
            }
            .as_u8() as u32;
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w9, #expected_tag
                ; b.ne >fail
            );
            match ty {
                ValueType::Float => {
                    if let Some(d) = self.direct_dest_d(value_dest) {
                        dynasm!(self.ops ; .arch aarch64 ; ldr D(d), [x10, 8]);
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; ldr d0, [x10, 8]);
                        self.store_d0_as_float(value_dest);
                    }
                }
                ValueType::Int => {
                    if let Some(d) = self.direct_dest_x(value_dest) {
                        dynasm!(self.ops ; .arch aarch64 ; ldr X(d), [x10, 8]);
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x10, 8]);
                        self.store_from_x0(value_dest, ValueTag::Int.as_u8());
                        self.hot_x0 = Some(value_dest);
                    }
                }
                _ => {
                    if let Some(d) = self.direct_dest_x(value_dest) {
                        dynasm!(self.ops ; .arch aarch64 ; ldrb W(d), [x10, 8]);
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; ldrb w0, [x10, 8]);
                        self.store_from_x0(value_dest, ValueTag::Bool.as_u8());
                        self.hot_x0 = Some(value_dest);
                    }
                }
            }
            self.pending_scalar = Some((value_dest, ty));
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn compile_array_len(&mut self, dest: u8, array: u8) -> Result<()> {
        let array_tag = ValueTag::Array.as_u8() as u32;
        unsafe extern "C" {
            fn jit_array_len_safe(array_value: *const Value) -> i64;
        }

        if let Some(hoisted) = self.hoisted_array
            && hoisted.array_reg == array
            && let Some(len_r) = hoisted.len_reg
        {
            dynasm!(self.ops
                ; .arch aarch64
                ; mov x0, X(len_r)
            );
            self.store_from_x0(dest, ValueTag::Int.as_u8());
            self.hot_x0 = Some(dest);
            self.pending_scalar = Some((dest, ValueType::Int));
            return Ok(());
        }

        self.load_tag(0, array);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w0, #array_tag
            ; b.ne >fail
        );
        self.emit_reg_addr(0, array);
        self.emit_call(jit_array_len_safe as *const ());
        // `tbnz` only reaches ±32 KB; traces can be larger, so branch on
        // the flags instead (`b.cond` reaches ±1 MB like every other exit).
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp x0, 0
            ; b.lt >fail
        );
        self.store_from_x0(dest, ValueTag::Int.as_u8());
        Ok(())
    }

    pub(super) fn compile_get_field(
        &mut self,
        dest: u8,
        object: u8,
        field_name: &str,
        field_index: Option<usize>,
        _value_type: Option<ValueType>,
        _is_weak: bool,
    ) -> Result<()> {
        unsafe extern "C" {
            fn jit_get_field_keyed(
                object_ptr: *const Value,
                field_name_ptr: *const u8,
                field_name_len: usize,
                key: *const crate::bytecode::ValueKey,
                out: *mut Value,
            ) -> u8;
            fn jit_get_field_indexed_safe(
                object_ptr: *const Value,
                field_index: usize,
                out: *mut Value,
            ) -> u8;
        }

        if let (Some(index), Some(ty), Some(layout), false) = (
            field_index,
            _value_type
                .filter(|ty| matches!(ty, ValueType::Int | ValueType::Float | ValueType::Bool)),
            jit::layout::rc_vec_layout(),
            _is_weak,
        ) {
            // A scalar field read inline: struct tag, field count, element
            // tag are each checked, and anything unexpected fails to the
            // interpreter, which re-executes the read. The field is a
            // scalar of the recorded type afterwards.
            let expected_tag = match ty {
                ValueType::Int => ValueTag::Int,
                ValueType::Float => ValueTag::Float,
                _ => ValueTag::Bool,
            }
            .as_u8() as u32;
            let struct_tag = ValueTag::Struct.as_u8() as u32;
            let fields_offset = layout.struct_fields_offset as u32;
            let len_offset = layout.struct_len_offset as u32;
            let ptr_offset = layout.struct_ptr_offset as u32;
            let element = (index * mem::size_of::<Value>()) as i32;
            let is_guarded = self.guarded_structs.contains(&object);
            if !is_guarded {
                self.load_tag(0, object);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w0, #struct_tag
                    ; b.ne >fail
                );
            }
            self.emit_reg_addr(11, object);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x9, [x11, #fields_offset]
            );
            if !is_guarded {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x10, [x9, #len_offset]
                    ; cmp x10, #index as u32
                    ; b.ls >fail
                );
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x10, [x9, #ptr_offset]
            );
            self.emit_add_imm(10, 10, element);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldrb w9, [x10]
                ; cmp w9, #expected_tag
                ; b.ne >fail
            );
            match ty {
                ValueType::Float => {
                    dynasm!(self.ops ; .arch aarch64 ; ldr d0, [x10, 8]);
                    self.store_d0_as_float(dest);
                }
                ValueType::Int => {
                    dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x10, 8]);
                    self.store_from_x0(dest, ValueTag::Int.as_u8());
                }
                _ => {
                    dynasm!(self.ops ; .arch aarch64 ; ldrb w0, [x10, 8]);
                    self.store_from_x0(dest, ValueTag::Bool.as_u8());
                }
            }
            self.pending_scalar = Some((dest, ty));
            return Ok(());
        }

        if let (Some(index), Some(layout), false) =
            (field_index, jit::layout::rc_vec_layout(), _is_weak)
        {
            // Any strong field read inline: struct tag and field count are
            // checked (anything else fails to the interpreter), then the
            // element is cloned into the register without the runtime.
            let struct_tag = ValueTag::Struct.as_u8() as u32;
            let fields_offset = layout.struct_fields_offset as u32;
            let len_offset = layout.struct_len_offset as u32;
            let ptr_offset = layout.struct_ptr_offset as u32;
            let element = (index * mem::size_of::<Value>()) as i32;
            let is_guarded = self.guarded_structs.contains(&object);
            if !is_guarded {
                self.load_tag(0, object);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w0, #struct_tag
                    ; b.ne >fail
                );
            }
            self.emit_reg_addr(11, object);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x9, [x11, #fields_offset]
            );
            if !is_guarded {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x10, [x9, #len_offset]
                    ; cmp x10, #index as u32
                    ; b.ls >fail
                );
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x11, [x9, #ptr_offset]
            );
            self.emit_add_imm(11, 11, element);
            self.emit_clone_x11_into(dest);
            return Ok(());
        }

        if let Some(index) = field_index {
            self.emit_reg_addr(0, object);
            self.emit_mov_imm64(1, index as u64);
            self.emit_reg_addr(2, dest);
            self.emit_call(jit_get_field_indexed_safe as *const ());
        } else {
            let (field_name_ptr, field_name_len) = self.retain_string(field_name);
            let key = self.retain_key(field_name);
            self.emit_reg_addr(0, object);
            self.emit_mov_imm64(1, field_name_ptr as usize as u64);
            self.emit_mov_imm64(2, field_name_len as u64);
            self.emit_mov_imm64(3, key as usize as u64);
            self.emit_reg_addr(4, dest);
            self.emit_call(jit_get_field_keyed as *const ());
        }
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    pub(super) fn compile_set_field(
        &mut self,
        object: u8,
        field_name: &str,
        value: u8,
        field_index: Option<usize>,
        _value_type: Option<ValueType>,
        _is_weak: bool,
    ) -> Result<()> {
        unsafe extern "C" {
            fn jit_set_field_safe(
                object_ptr: *const Value,
                field_name_ptr: *const u8,
                field_name_len: usize,
                value_ptr: *const Value,
            ) -> u8;
            fn jit_set_field_indexed_safe(
                object_ptr: *const Value,
                field_index: usize,
                value_ptr: *const Value,
            ) -> u8;
            fn jit_set_field_strong_safe(
                object_ptr: *const Value,
                field_index: usize,
                value_ptr: *const Value,
            ) -> u8;
        }

        let value_ty = self
            .scalar_registers
            .get(&value)
            .copied()
            .or(_value_type)
            .filter(|ty| matches!(ty, ValueType::Int | ValueType::Float | ValueType::Bool));
        if let (Some(index), Some(ty), Some(layout), false) = (
            field_index,
            value_ty,
            jit::layout::rc_vec_layout(),
            _is_weak,
        ) {
            // A scalar store inline when the field currently holds the same
            // scalar kind and the struct is not borrowed; any other
            // situation goes through the helper, which does the full
            // canonicalization and borrow handling.
            let expected_tag = match ty {
                ValueType::Int => ValueTag::Int,
                ValueType::Float => ValueTag::Float,
                _ => ValueTag::Bool,
            }
            .as_u8() as u32;
            let struct_tag = ValueTag::Struct.as_u8() as u32;
            let fields_offset = layout.struct_fields_offset as u32;
            let len_offset = layout.struct_len_offset as u32;
            let ptr_offset = layout.struct_ptr_offset as u32;
            let borrow_offset = layout.struct_borrow_offset as u32;
            let element = (index * mem::size_of::<Value>()) as i32;
            let done = self.ops.new_dynamic_label();
            self.load_tag(0, object);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w0, #struct_tag
                ; b.ne >slow
            );
            self.emit_reg_addr(11, object);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x9, [x11, #fields_offset]
                ; ldr x10, [x9, #borrow_offset]
                ; cbnz x10, >slow
                ; ldr x10, [x9, #len_offset]
                ; cmp x10, #index as u32
                ; b.ls >slow
                ; ldr x10, [x9, #ptr_offset]
            );
            self.emit_add_imm(10, 10, element);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldrb w9, [x10]
                ; cmp w9, #expected_tag
                ; b.ne >slow
            );
            match ty {
                ValueType::Float => {
                    self.load_payload_f(0, value);
                    dynasm!(self.ops ; .arch aarch64 ; str d0, [x10, 8]);
                }
                ValueType::Int => {
                    self.load_payload(0, value);
                    dynasm!(self.ops ; .arch aarch64 ; str x0, [x10, 8]);
                }
                _ => {
                    self.load_bool_payload(0, value);
                    dynasm!(self.ops ; .arch aarch64 ; strb w0, [x10, 8]);
                }
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; b => done
                ; slow:
            );
            self.emit_reg_addr(0, object);
            self.emit_mov_imm64(1, index as u64);
            self.emit_reg_addr(2, value);
            self.emit_call(jit_set_field_indexed_safe as *const ());
            self.emit_fail_if_w0_zero();
            dynasm!(self.ops ; .arch aarch64 ; => done);
            return Ok(());
        }

        if let Some(index) = field_index {
            crate::jit::log(|| {
                format!(
                    "🔧 JIT: SetField using indexed path, field_index={}, is_weak={}",
                    index, _is_weak
                )
            });
            // Weak fields need canonicalization; strong fields can skip it.
            let helper: *const () = if _is_weak {
                jit_set_field_indexed_safe as *const ()
            } else {
                jit_set_field_strong_safe as *const ()
            };
            self.emit_reg_addr(0, object);
            self.emit_mov_imm64(1, index as u64);
            self.emit_reg_addr(2, value);
            self.emit_call(helper);
        } else {
            let (field_name_ptr, field_name_len) = self.retain_string(field_name);
            self.emit_reg_addr(0, object);
            self.emit_mov_imm64(1, field_name_ptr as usize as u64);
            self.emit_mov_imm64(2, field_name_len as u64);
            self.emit_reg_addr(3, value);
            self.emit_call(jit_set_field_safe as *const ());
        }
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    /// Sink a promoted field. Skip when the register is not the scalar the
    /// entry load would have left there: a preamble failure never wrote it,
    /// and storing Nil would clobber the field.
    pub(super) fn compile_restore_field(
        &mut self,
        object: u8,
        field_name: &str,
        value: u8,
        field_index: usize,
        value_type: ValueType,
    ) -> Result<()> {
        let expected = match value_type {
            ValueType::Int => ValueTag::Int,
            ValueType::Float => ValueTag::Float,
            ValueType::Bool => ValueTag::Bool,
            _ => ValueTag::Int,
        }
        .as_u8() as u32;
        let skip = self.ops.new_dynamic_label();
        self.load_tag(0, value);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w0, #expected
            ; b.ne => skip
        );
        self.compile_set_field(
            object,
            field_name,
            value,
            Some(field_index),
            Some(value_type),
            false,
        )?;
        dynasm!(self.ops ; .arch aarch64 ; => skip);
        Ok(())
    }

    pub(super) fn compile_new_array(
        &mut self,
        dest: u8,
        first_element: u8,
        count: u8,
    ) -> Result<()> {
        unsafe extern "C" {
            fn jit_new_array_safe(
                vm_ptr: *mut crate::VM,
                elements_ptr: *const Value,
                element_count: usize,
                out: *mut Value,
            ) -> u8;
        }

        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, first_element); // ignored by the helper if count == 0
        self.emit_mov_imm64(2, count as u64);
        self.emit_reg_addr(3, dest);
        self.emit_call(jit_new_array_safe as *const ());
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    /// `array.push(array, value)` / `array:push(value)`. Inline when the
    /// array has spare capacity, is not borrowed, and the value owns
    /// nothing: the value's two words go after the last element and the
    /// length grows. Growth (the memory budget is charged), a borrowed
    /// array or an owning value go through the helper, which fails to the
    /// interpreter for an error.
    pub(super) fn compile_array_push(&mut self, array: u8, value: u8) -> Result<()> {
        unsafe extern "C" {
            fn jit_array_push_safe(
                vm_ptr: *mut crate::VM,
                array_ptr: *const Value,
                value_ptr: *const Value,
            ) -> u8;
        }

        if let Some(layout) = jit::layout::rc_vec_layout() {
            let array_tag = ValueTag::Array.as_u8() as u32;
            let scalar_max_tag = ValueTag::Float.as_u8() as u32;
            let rc_offset = layout.array_rc_offset as u32;
            let borrow_offset = layout.borrow_offset as u32;
            let len_offset = layout.len_offset as u32;
            let cap_offset = layout.cap_offset as u32;
            let ptr_offset = layout.ptr_offset as u32;
            let _value_size = mem::size_of::<Value>() as u32;
            let done = self.ops.new_dynamic_label();
            let slow = self.ops.new_dynamic_label();

            if let Some(hoisted) = self.hoisted_array
                && hoisted.array_reg == array
            {
                let rc = hoisted.rc_reg;
                let ptr = hoisted.ptr_reg;
                let len_r_opt = hoisted.len_reg;
                if let Some(len_r) = len_r_opt {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x10, [X(rc), #cap_offset]
                        ; cmp X(len_r), x10
                        ; b.hs => slow
                    );
                } else {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x12, [X(rc), #len_offset]
                        ; ldr x10, [X(rc), #cap_offset]
                        ; cmp x12, x10
                        ; b.hs => slow
                    );
                }
                if let Some(ty) = self.scalar_registers.get(&value).copied() {
                    let tag = match ty {
                        ValueType::Int => ValueTag::Int.as_u8(),
                        ValueType::Float => ValueTag::Float.as_u8(),
                        _ => ValueTag::Bool.as_u8(),
                    } as u32;
                    if let Some(pin) = self.active_pin(value) {
                        dynasm!(self.ops ; .arch aarch64 ; mov X(1), X(pin.reg));
                    } else if let Some(c) = self.const_registers.get(&value).cloned() {
                        match c {
                            Value::Bool(b) => {
                                let b_val = if b { 1u32 } else { 0u32 };
                                dynasm!(self.ops ; .arch aarch64 ; movz x1, #b_val);
                            }
                            Value::Int(i) => {
                                self.emit_mov_imm64(1, i as u64);
                            }
                            Value::Float(f) => {
                                self.emit_mov_imm64(1, f.to_bits());
                            }
                            _ => {
                                if ty == ValueType::Bool {
                                    self.load_bool_payload(1, value);
                                } else {
                                    self.load_payload(1, value);
                                }
                            }
                        }
                    } else if ty == ValueType::Bool {
                        self.load_bool_payload(1, value);
                    } else {
                        self.load_payload(1, value);
                    }
                    if let Some(len_r) = len_r_opt {
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; movz x0, #tag
                            ; add x10, X(ptr), X(len_r), lsl #4
                            ; stp x0, x1, [x10]
                            ; add XSP(len_r), XSP(len_r), 1
                            ; b => done
                            ; => slow
                        );
                    } else {
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; movz x0, #tag
                            ; add x10, X(ptr), x12, lsl #4
                            ; stp x0, x1, [x10]
                            ; add x12, x12, #1
                            ; str x12, [X(rc), #len_offset]
                            ; b => done
                            ; => slow
                        );
                    }
                } else {
                    self.load_tag_from_memory(13, value);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; cmp w13, #scalar_max_tag
                        ; b.hi => slow
                    );
                    self.emit_reg_addr(11, value);
                    if let Some(len_r) = len_r_opt {
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; add x10, X(ptr), X(len_r), lsl #4
                            ; ldp x0, x1, [x11]
                            ; stp x0, x1, [x10]
                            ; add XSP(len_r), XSP(len_r), 1
                            ; b => done
                            ; => slow
                        );
                    } else {
                        dynasm!(self.ops
                            ; .arch aarch64
                            ; add x10, X(ptr), x12, lsl #4
                            ; ldp x0, x1, [x11]
                            ; stp x0, x1, [x10]
                            ; add x12, x12, #1
                            ; str x12, [X(rc), #len_offset]
                            ; b => done
                            ; => slow
                        );
                    }
                }
                if let Some(len_r) = len_r_opt {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; str X(len_r), [X(rc), #len_offset]
                    );
                }
                dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
                self.emit_reg_addr(1, array);
                self.emit_reg_addr(2, value);
                self.emit_call(jit_array_push_safe as *const ());
                self.emit_fail_if_w0_zero();
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr X(ptr), [X(rc), #ptr_offset]
                );
                if let Some(len_r) = len_r_opt {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr X(len_r), [X(rc), #len_offset]
                    );
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; => done
                );
                return Ok(());
            }
            if !self.array_registers.contains(&array) {
                self.load_tag(0, array);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w0, #array_tag
                    ; b.ne => slow
                );
                self.array_registers.insert(array);
            }
            self.emit_reg_addr(11, array);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x9, [x11, #rc_offset]
                ; ldr x10, [x9, #borrow_offset]
                ; cbnz x10, => slow
                ; ldr x12, [x9, #len_offset]
                ; ldr x10, [x9, #cap_offset]
                ; cmp x12, x10
                ; b.hs => slow
            );
            if let Some(ty) = self.scalar_registers.get(&value).copied() {
                let tag = match ty {
                    ValueType::Int => ValueTag::Int.as_u8(),
                    ValueType::Float => ValueTag::Float.as_u8(),
                    _ => ValueTag::Bool.as_u8(),
                } as u32;
                if let Some(pin) = self.active_pin(value) {
                    dynasm!(self.ops ; .arch aarch64 ; mov X(1), X(pin.reg));
                } else if let Some(c) = self.const_registers.get(&value).cloned() {
                    match c {
                        Value::Bool(b) => {
                            let b_val = if b { 1u32 } else { 0u32 };
                            dynasm!(self.ops ; .arch aarch64 ; movz x1, #b_val);
                        }
                        Value::Int(i) => {
                            self.emit_mov_imm64(1, i as u64);
                        }
                        Value::Float(f) => {
                            self.emit_mov_imm64(1, f.to_bits());
                        }
                        _ => {
                            if ty == ValueType::Bool {
                                self.load_bool_payload(1, value);
                            } else {
                                self.load_payload(1, value);
                            }
                        }
                    }
                } else if ty == ValueType::Bool {
                    self.load_bool_payload(1, value);
                } else {
                    self.load_payload(1, value);
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; movz x0, #tag
                    ; ldr x10, [x9, #ptr_offset]
                    ; add x10, x10, x12, lsl #4
                    ; stp x0, x1, [x10]
                    ; add x12, x12, #1
                    ; str x12, [x9, #len_offset]
                    ; b => done
                    ; => slow
                );
            } else {
                self.load_tag_from_memory(13, value);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w13, #scalar_max_tag
                    ; b.hi => slow
                );
                self.emit_reg_addr(11, value);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x10, [x9, #ptr_offset]
                    ; add x10, x10, x12, lsl #4
                    ; ldp x0, x1, [x11]
                    ; stp x0, x1, [x10]
                    ; add x12, x12, #1
                    ; str x12, [x9, #len_offset]
                    ; b => done
                    ; => slow
                );
            }
            dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
            self.emit_reg_addr(1, array);
            self.emit_reg_addr(2, value);
            self.emit_call(jit_array_push_safe as *const ());
            self.emit_fail_if_w0_zero();
            dynasm!(self.ops ; .arch aarch64 ; => done);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
            self.emit_reg_addr(1, array);
            self.emit_reg_addr(2, value);
            self.emit_call(jit_array_push_safe as *const ());
            self.emit_fail_if_w0_zero();
        }
        Ok(())
    }

    pub(super) fn compile_enum_is_some(&mut self, dest: u8, enum_reg: u8) -> Result<()> {
        unsafe extern "C" {
            fn jit_enum_is_some_safe(enum_ptr: *const Value, out_ptr: *mut Value) -> u8;
        }

        self.emit_reg_addr(0, enum_reg);
        self.emit_reg_addr(1, dest);
        self.emit_call(jit_enum_is_some_safe as *const ());
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    pub(super) fn compile_enum_unwrap(&mut self, dest: u8, enum_reg: u8) -> Result<()> {
        unsafe extern "C" {
            fn jit_enum_unwrap_safe(
                vm_ptr: *mut crate::VM,
                enum_ptr: *const Value,
                out_ptr: *mut Value,
            ) -> u8;
        }

        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, enum_reg);
        self.emit_reg_addr(2, dest);
        self.emit_call(jit_enum_unwrap_safe as *const ());
        self.emit_fail_if_w0_zero();

        Ok(())
    }

    pub(super) fn compile_call_native(
        &mut self,
        dest: u8,
        callee: u8,
        expected_ptr: *const (),
        first_arg: u8,
        arg_count: u8,
    ) -> Result<()> {
        let exit_label = self.current_exit_label();
        unsafe extern "C" {
            fn jit_call_native_safe(
                vm_ptr: *mut crate::VM,
                callee_ptr: *const Value,
                expected: *const (),
                args_ptr: *const Value,
                arg_count: u8,
                out: *mut Value,
            ) -> u8;
        }

        self.emit_call_ip();
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, callee);
        self.emit_mov_imm64(2, expected_ptr as usize as u64);
        self.emit_reg_addr(3, first_arg);
        self.emit_mov_imm32(4, arg_count as u32);
        self.emit_reg_addr(5, dest);
        self.emit_call(jit_call_native_safe as *const ());
        // 0 = failure, 1 = ok, 2/3 = yield/task signal (exit with that code).
        dynasm!(self.ops
            ; .arch aarch64
            ; and w0, w0, 0xff
            ; cmp w0, 2
            ; b.eq >native_yield
            ; cmp w0, 3
            ; b.eq >native_yield
            ; cbz w0, >fail
            ; b >native_ok
            ; native_yield:
            ; b => exit_label
            ; native_ok:
        );

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_call_function(
        &mut self,
        dest: u8,
        callee: u8,
        _function_idx: usize,
        first_arg: u8,
        arg_count: u8,
        is_closure: bool,
        upvalues_ptr: Option<*const ()>,
    ) -> Result<()> {
        let _ = (_function_idx, is_closure, upvalues_ptr);
        unsafe extern "C" {
            fn jit_call_function_safe(
                vm_ptr: *mut crate::VM,
                callee_ptr: *const Value,
                args_ptr: *const Value,
                arg_count: u8,
                dest_reg: u8,
                out: *mut Value,
            ) -> u8;
        }

        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, callee);
        self.emit_reg_addr(2, first_arg);
        self.emit_mov_imm32(3, arg_count as u32);
        self.emit_mov_imm32(4, dest as u32);
        self.emit_call_result_out(5, dest);
        self.emit_call_ip();
        self.emit_call(jit_call_function_safe as *const ());
        self.emit_fail_if_w0_zero();
        self.emit_reload_registers_base();
        Ok(())
    }

    /// Where a call helper should deliver its result. Inside an inlined
    /// frame the destination register lives on the JIT stack, so its address
    /// is stable and is passed directly. At depth zero the VM may reallocate
    /// its register file during the call, so the helper writes the result by
    /// index into the current frame instead (and x19 is reloaded after).
    /// `JIT_CALL_IP = ip of the call being compiled`, for the helper about
    /// to run it (unknown inside an inlined body, whose frame the
    /// interpreter does not have). Clobbers x11/x12 only.
    fn emit_call_ip(&mut self) {
        let ip = if self.inline_depth == 0 {
            self.current_fail_ip.unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };
        let offset = jit::CALL_IP_OFFSET as u32;
        self.emit_mov_imm64(12, ip as u64);
        dynasm!(self.ops ; .arch aarch64 ; str x12, [x20, #offset]);
    }

    fn emit_call_result_out(&mut self, x: u8, dest: u8) {
        // Function code's frame is wherever x19 points (the machine stack
        // when called natively), never looked up through the VM.
        if self.inline_depth > 0 || self.function_mode {
            self.emit_reg_addr(x, dest);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; mov X(x), xzr);
        }
    }

    /// The VM may have reallocated its register file during a call; refresh
    /// x19 from the VM unless we are inside an inlined frame (whose registers
    /// live on our own stack).
    fn emit_reload_registers_base(&mut self) {
        unsafe extern "C" {
            fn jit_current_registers(vm_ptr: *mut crate::VM) -> *mut Value;
        }
        if self.inline_depth == 0 && !self.function_mode {
            dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
            self.emit_call(jit_current_registers as *const ());
            dynasm!(self.ops
                ; .arch aarch64
                ; cbz x0, >fail
                ; mov x19, x0
            );
        }
    }

    pub(super) fn compile_call_method(
        &mut self,
        dest: u8,
        object: u8,
        method_name: &str,
        first_arg: u8,
        arg_count: u8,
    ) -> Result<()> {
        unsafe extern "C" {
            fn jit_call_method_safe(
                vm_ptr: *mut crate::VM,
                object_ptr: *const Value,
                method_name_ptr: *const u8,
                method_name_len: usize,
                args_ptr: *const Value,
                arg_count: u8,
                dest_reg: u8,
                out: *mut Value,
            ) -> u8;
        }

        let (method_name_ptr, method_name_len) = self.retain_string(method_name);
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, object);
        self.emit_mov_imm64(2, method_name_ptr as usize as u64);
        self.emit_mov_imm64(3, method_name_len as u64);
        self.emit_reg_addr(4, first_arg);
        self.emit_mov_imm32(5, arg_count as u32);
        self.emit_mov_imm32(6, dest as u32);
        self.emit_call_result_out(7, dest);
        self.emit_call(jit_call_method_safe as *const ());
        self.emit_fail_if_w0_zero();
        self.emit_reload_registers_base();
        Ok(())
    }

    pub(super) fn compile_new_struct(
        &mut self,
        dest: u8,
        struct_name: &str,
        field_names: &[String],
        field_registers: &[u8],
    ) -> Result<()> {
        unsafe extern "C" {
            fn jit_new_struct_cached(
                vm_ptr: *mut crate::VM,
                site_ptr: *mut crate::jit::JitStructSite,
                field_values_ptr: *const Value,
                field_count: usize,
                out: *mut Value,
            ) -> u8;
        }

        let site_ptr = self.retain_struct_site(struct_name, field_names);
        let field_count = field_names.len();
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_mov_imm64(1, site_ptr as usize as u64);
        if let Some(first) = field_registers.first() {
            self.emit_reg_addr(2, *first);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; mov x2, xzr);
        }
        self.emit_mov_imm64(3, field_count as u64);
        self.emit_reg_addr(4, dest);
        self.emit_call(jit_new_struct_cached as *const ());
        self.emit_fail_if_w0_zero();
        self.emit_reload_registers_base();
        Ok(())
    }

    pub(super) fn compile_store_global(&mut self, name: &str, src: u8) -> Result<()> {
        unsafe extern "C" {
            fn jit_set_global_safe(
                vm_ptr: *mut crate::VM,
                name_ptr: *const u8,
                name_len: usize,
                value_ptr: *const Value,
            ) -> u8;
        }

        let (name_ptr, name_len) = self.retain_string(name);
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_mov_imm64(1, name_ptr as usize as u64);
        self.emit_mov_imm64(2, name_len as u64);
        self.emit_reg_addr(3, src);
        self.emit_call(jit_set_global_safe as *const ());
        self.emit_fail_if_w0_zero();
        self.emit_reload_registers_base();
        Ok(())
    }

    pub(super) fn compile_new_enum_unit(
        &mut self,
        dest: u8,
        enum_name: &str,
        variant_name: &str,
    ) -> Result<()> {
        // One shared unit value per site: the store is a copy and a count
        // bump.
        let unit = self.retain_value(Value::enum_unit(enum_name, variant_name));
        self.emit_mov_imm64(11, unit as usize as u64);
        self.emit_clone_x11_into(dest);
        Ok(())
    }

    pub(super) fn compile_new_enum_variant(
        &mut self,
        dest: u8,
        enum_name: &str,
        variant_name: &str,
        value_registers: &[u8],
    ) -> Result<()> {
        if enum_name == "Option" && variant_name == "Some" && value_registers.len() == 1 {
            unsafe extern "C" {
                fn jit_new_option_some_fast(
                    vm_ptr: *mut crate::VM,
                    value_ptr: *const Value,
                    out: *mut Value,
                ) -> u8;
            }
            dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
            self.emit_reg_addr(1, value_registers[0]);
            self.emit_reg_addr(2, dest);
            self.emit_call(jit_new_option_some_fast as *const ());
            self.emit_fail_if_w0_zero();
            return Ok(());
        }

        unsafe extern "C" {
            fn jit_new_enum_variant_safe(
                vm_ptr: *mut crate::VM,
                enum_name_ptr: *const u8,
                enum_name_len: usize,
                variant_name_ptr: *const u8,
                variant_name_len: usize,
                values_ptr: *const Value,
                value_count: usize,
                out: *mut Value,
            ) -> u8;
        }
        let (enum_name_ptr, enum_name_len) = self.retain_string(enum_name);
        let (variant_name_ptr, variant_name_len) = self.retain_string(variant_name);
        let (first_value, value_count) = match value_registers.first() {
            Some(first) => (*first, value_registers.len()),
            None => (0, 0),
        };
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_mov_imm64(1, enum_name_ptr as usize as u64);
        self.emit_mov_imm64(2, enum_name_len as u64);
        self.emit_mov_imm64(3, variant_name_ptr as usize as u64);
        self.emit_mov_imm64(4, variant_name_len as u64);
        self.emit_reg_addr(5, first_value);
        self.emit_mov_imm64(6, value_count as u64);
        self.emit_reg_addr(7, dest);
        self.emit_call(jit_new_enum_variant_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }

    /// Returns whether the test was compiled inline (the enum's tag and
    /// names compared), so a following branch knows what a true result
    /// proves.
    pub(super) fn compile_is_enum_variant(
        &mut self,
        dest: u8,
        value: u8,
        enum_name: &str,
        variant_name: &str,
    ) -> Result<bool> {
        unsafe extern "C" {
            fn jit_is_enum_variant_safe(
                value_ptr: *const Value,
                enum_name_ptr: *const u8,
                enum_name_len: usize,
                variant_name_ptr: *const u8,
                variant_name_len: usize,
            ) -> u8;
        }
        if let Some(layout) = jit::layout::enum_layout() {
            // Names are interned, so the variant test is a tag check and
            // pointer compares against the interned constants (the enum
            // name too, unless the test leaves it open).
            let variant_ptr = self.retain_name(variant_name);
            let enum_ptr = (!enum_name.is_empty()).then(|| self.retain_name(enum_name));
            let tag = layout.tag as u32;
            let object_offset = layout.object_offset as u32;
            let variant_offset = layout.variant_offset as u32;
            let enum_name_offset = layout.enum_name_offset as u32;
            self.load_tag(0, value);
            self.emit_reg_addr(11, value);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w0, #tag
                ; b.ne >no
                ; ldr x11, [x11, #object_offset]
                ; ldr x9, [x11, #variant_offset]
            );
            self.emit_mov_imm64(10, variant_ptr as u64);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp x9, x10
                ; b.ne >no
            );
            if let Some(enum_ptr) = enum_ptr {
                dynasm!(self.ops ; .arch aarch64 ; ldr x9, [x11, #enum_name_offset]);
                self.emit_mov_imm64(10, enum_ptr as u64);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp x9, x10
                    ; b.ne >no
                );
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; movz x0, 1
                ; b >done
                ; no:
                ; mov x0, xzr
                ; done:
            );
            self.store_from_x0(dest, ValueTag::Bool.as_u8());
            return Ok(true);
        }
        let (enum_name_ptr, enum_name_len) = self.retain_string(enum_name);
        let (variant_name_ptr, variant_name_len) = self.retain_string(variant_name);
        self.emit_reg_addr(0, value);
        self.emit_mov_imm64(1, enum_name_ptr as usize as u64);
        self.emit_mov_imm64(2, enum_name_len as u64);
        self.emit_mov_imm64(3, variant_name_ptr as usize as u64);
        self.emit_mov_imm64(4, variant_name_len as u64);
        self.emit_call(jit_is_enum_variant_safe as *const ());
        dynasm!(self.ops ; .arch aarch64 ; and x0, x0, 0xff);
        self.store_from_x0(dest, ValueTag::Bool.as_u8());
        Ok(false)
    }

    pub(super) fn compile_is_enum_variant_branch(
        &mut self,
        dest: u8,
        value: u8,
        enum_name: &str,
        variant_name: &str,
        label: usize,
        dead_dest: bool,
    ) -> Result<bool> {
        let Some(layout) = jit::layout::enum_layout() else {
            return Ok(false);
        };
        let variant_ptr = self.retain_name(variant_name);
        let enum_ptr = (!enum_name.is_empty()).then(|| self.retain_name(enum_name));
        let tag = layout.tag as u32;
        let object_offset = layout.object_offset as u32;
        let variant_offset = layout.variant_offset as u32;
        let enum_name_offset = layout.enum_name_offset as u32;
        let target = self.function_label(label);

        self.emit_back_edge_gas_check(label);
        self.load_tag(0, value);
        self.emit_reg_addr(11, value);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w0, #tag
            ; b.ne => target
            ; ldr x11, [x11, #object_offset]
            ; ldr x9, [x11, #variant_offset]
        );
        self.emit_mov_imm64(10, variant_ptr as u64);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp x9, x10
            ; b.ne => target
        );
        if let Some(enum_ptr) = enum_ptr {
            dynasm!(self.ops ; .arch aarch64 ; ldr x9, [x11, #enum_name_offset]);
            self.emit_mov_imm64(10, enum_ptr as u64);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp x9, x10
                ; b.ne => target
            );
        }
        if !dead_dest {
            dynasm!(self.ops ; .arch aarch64 ; movz x0, 1);
            self.store_from_x0(dest, ValueTag::Bool.as_u8());
        }
        Ok(true)
    }

    pub(super) fn compile_type_is(&mut self, dest: u8, value: u8, type_name: &str) -> Result<()> {
        let direct_tag = match type_name {
            "nil" | "()" => Some(ValueTag::Nil),
            "bool" => Some(ValueTag::Bool),
            "int" => Some(ValueTag::Int),
            "float" => Some(ValueTag::Float),
            "string" => Some(ValueTag::String),
            "Array" => Some(ValueTag::Array),
            "Tuple" => Some(ValueTag::Tuple),
            "Map" => Some(ValueTag::Map),
            _ => None,
        };
        if type_name == "unknown" {
            dynasm!(self.ops ; .arch aarch64 ; movz x0, 1);
            self.store_from_x0(dest, ValueTag::Bool.as_u8());
            return Ok(());
        }
        if let Some(tag) = direct_tag {
            let tag = tag.as_u8() as u32;
            self.load_tag(9, value);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w9, #tag
                ; cset x0, eq
            );
            self.store_from_x0(dest, ValueTag::Bool.as_u8());
            return Ok(());
        }

        unsafe extern "C" {
            fn jit_type_is_safe(
                vm_ptr: *mut crate::VM,
                value_ptr: *const Value,
                type_name_ptr: *const u8,
                type_name_len: usize,
            ) -> u8;
        }
        let (type_name_ptr, type_name_len) = self.retain_string(type_name);
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, value);
        self.emit_mov_imm64(2, type_name_ptr as usize as u64);
        self.emit_mov_imm64(3, type_name_len as u64);
        self.emit_call(jit_type_is_safe as *const ());
        dynasm!(self.ops ; .arch aarch64 ; and x0, x0, 0xff);
        self.store_from_x0(dest, ValueTag::Bool.as_u8());
        Ok(())
    }

    pub(super) fn compile_try_cast(&mut self, dest: u8, value: u8, type_name: &str) -> Result<()> {
        unsafe extern "C" {
            fn jit_try_cast_safe(
                vm_ptr: *mut crate::VM,
                value_ptr: *const Value,
                type_name_ptr: *const u8,
                type_name_len: usize,
                out: *mut Value,
            ) -> u8;
        }
        let (type_name_ptr, type_name_len) = self.retain_string(type_name);
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x20);
        self.emit_reg_addr(1, value);
        self.emit_mov_imm64(2, type_name_ptr as usize as u64);
        self.emit_mov_imm64(3, type_name_len as u64);
        self.emit_reg_addr(4, dest);
        self.emit_call(jit_try_cast_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }

    /// `dest` = the bits of strong field `index` of the struct in `object`,
    /// with no count taken and nothing released (see
    /// `TraceOp::BorrowField`). The struct tag and field count are checked;
    /// anything else fails to the interpreter, which re-executes the read.
    pub(super) fn compile_borrow_field(
        &mut self,
        dest: u8,
        object: u8,
        index: usize,
    ) -> Result<()> {
        let Some(layout) = jit::layout::rc_vec_layout() else {
            return Err(crate::LustError::RuntimeError {
                message: "a borrow needs the measured struct layout".into(),
            });
        };
        let struct_tag = ValueTag::Struct.as_u8() as u32;
        let fields_offset = layout.struct_fields_offset as u32;
        let len_offset = layout.struct_len_offset as u32;
        let ptr_offset = layout.struct_ptr_offset as u32;
        let element = (index * mem::size_of::<Value>()) as i32;
        let is_guarded = self.guarded_structs.contains(&object);
        if !is_guarded {
            self.load_tag(0, object);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w0, #struct_tag
                ; b.ne >fail
            );
        }
        self.emit_reg_addr(11, object);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x9, [x11, #fields_offset]
        );
        if !is_guarded {
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x10, [x9, #len_offset]
                ; cmp x10, #index as u32
                ; b.ls >fail
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x11, [x9, #ptr_offset]
        );
        self.emit_add_imm(11, 11, element);
        self.emit_reg_addr(9, dest);
        self.emit_copy_x11_to_x9();
        Ok(())
    }

    /// Read field `index` of `object`. If it is `Option::Some`, borrow its
    /// payload[0] directly into `dest`. If not, branch to `label`.
    pub(super) fn compile_borrow_field_some(
        &mut self,
        dest: u8,
        object: u8,
        index: usize,
        label: usize,
    ) -> Result<()> {
        let Some(layout) = jit::layout::rc_vec_layout() else {
            return Err(crate::LustError::RuntimeError {
                message: "a borrow needs the measured struct layout".into(),
            });
        };
        let Some(enum_layout) = jit::layout::enum_layout() else {
            return Err(crate::LustError::RuntimeError {
                message: "a borrow needs the measured enum layout".into(),
            });
        };
        let struct_tag = ValueTag::Struct.as_u8() as u32;
        let fields_offset = layout.struct_fields_offset as u32;
        let len_offset = layout.struct_len_offset as u32;
        let ptr_offset = layout.struct_ptr_offset as u32;
        let element = (index * mem::size_of::<Value>()) as i32;

        let enum_tag = enum_layout.tag as u32;
        let enum_object_offset = enum_layout.object_offset as u32;
        let variant_offset = enum_layout.variant_offset as u32;
        let values_ptr_offset = enum_layout.values_ptr_offset as u32;
        let variant_ptr = self.retain_name("Some");
        let target = self.function_label(label);

        self.emit_back_edge_gas_check(label);
        let is_guarded = self.guarded_structs.contains(&object);
        if !is_guarded {
            self.load_tag(0, object);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w0, #struct_tag
                ; b.ne => target
            );
        }
        self.emit_reg_addr(11, object);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x9, [x11, #fields_offset]
        );
        if !is_guarded {
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x10, [x9, #len_offset]
                ; cmp x10, #index as u32
                ; b.ls => target
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x11, [x9, #ptr_offset]
        );
        self.emit_add_imm(11, 11, element);
        self.emit_mov_imm64(12, variant_ptr as u64);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldrb w0, [x11]
            ; cmp w0, #enum_tag
            ; b.ne => target
            ; ldr x10, [x11, #enum_object_offset]
            ; ldr x13, [x10, #variant_offset]
            ; cmp x13, x12
            ; b.ne => target
            ; ldr x11, [x10, #values_ptr_offset]
        );
        self.emit_reg_addr(9, dest);
        self.emit_copy_x11_to_x9();
        Ok(())
    }

    /// `dest` = the bits of payload value `index` of the enum in
    /// `enum_reg`, a borrow of a borrow (see `TraceOp::BorrowEnumValue`).
    pub(super) fn compile_borrow_enum_value(
        &mut self,
        dest: u8,
        enum_reg: u8,
        index: u8,
    ) -> Result<()> {
        let Some(layout) = jit::layout::enum_layout() else {
            return Err(crate::LustError::RuntimeError {
                message: "a borrow needs the measured enum layout".into(),
            });
        };
        let tag = layout.tag as u32;
        let object_offset = layout.object_offset as u32;
        let len_offset = layout.values_len_offset as u32;
        let ptr_offset = layout.values_ptr_offset as u32;
        let unit_offset = layout.unit_word_offset as u32;
        let element = (index as usize * mem::size_of::<Value>()) as i32;
        // The enum test this read follows already proved the tag and the
        // variant; only the payload length is left to check.
        let verified = self.verified_enum_in == Some(enum_reg);
        if !verified {
            self.load_tag(0, enum_reg);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w0, #tag
                ; b.ne >fail
            );
        }
        self.emit_reg_addr(11, enum_reg);
        dynasm!(self.ops ; .arch aarch64 ; ldr x9, [x11, #object_offset]);
        if !verified {
            dynasm!(self.ops ; .arch aarch64 ; ldr x10, [x9, #unit_offset]);
            self.emit_mov_imm64(12, layout.unit_word_value as u64);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp x10, x12
                ; b.eq >fail
                ; ldr x10, [x9, #len_offset]
                ; cmp x10, #index as u32
                ; b.ls >fail
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x11, [x9, #ptr_offset]
        );
        self.emit_add_imm(11, 11, element);
        self.emit_reg_addr(9, dest);
        self.emit_copy_x11_to_x9();
        Ok(())
    }

    pub(super) fn compile_get_enum_value(
        &mut self,
        dest: u8,
        enum_reg: u8,
        index: u8,
    ) -> Result<()> {
        unsafe extern "C" {
            fn jit_get_enum_value_safe(enum_ptr: *const Value, index: usize, out: *mut Value)
            -> u8;
        }
        if let Some(layout) = jit::layout::enum_layout() {
            // Inline: an enum with a payload of at least `index + 1` values
            // (anything else fails to the interpreter), the value cloned
            // into the register without the runtime.
            let tag = layout.tag as u32;
            let object_offset = layout.object_offset as u32;
            let len_offset = layout.values_len_offset as u32;
            let ptr_offset = layout.values_ptr_offset as u32;
            let unit_offset = layout.unit_word_offset as u32;
            let element = (index as usize * mem::size_of::<Value>()) as i32;
            self.load_tag(0, enum_reg);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp w0, #tag
                ; b.ne >fail
            );
            self.emit_reg_addr(11, enum_reg);
            // A unit variant (no payload) fails: its niche word says so.
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x9, [x11, #object_offset]
                ; ldr x10, [x9, #unit_offset]
            );
            self.emit_mov_imm64(12, layout.unit_word_value as u64);
            dynasm!(self.ops
                ; .arch aarch64
                ; cmp x10, x12
                ; b.eq >fail
                ; ldr x10, [x9, #len_offset]
                ; cmp x10, #index as u32
                ; b.ls >fail
                ; ldr x11, [x9, #ptr_offset]
            );
            self.emit_add_imm(11, 11, element);
            self.emit_clone_x11_into(dest);
            return Ok(());
        }
        self.emit_reg_addr(0, enum_reg);
        self.emit_mov_imm64(1, index as u64);
        self.emit_reg_addr(2, dest);
        self.emit_call(jit_get_enum_value_safe as *const ());
        self.emit_fail_if_w0_zero();
        Ok(())
    }
}
