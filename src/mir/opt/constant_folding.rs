use crate::mir::ir::*;
use alloc::collections::BTreeMap;

pub struct ConstantFolding;

impl ConstantFolding {
    pub fn run_on_function(func: &mut MirFunction) -> bool {
        let mut changed = false;

        for block in &mut func.blocks {
            let mut constants: BTreeMap<LocalId, MirConstant> = BTreeMap::new();
            for stmt in &mut block.statements {
                match stmt {
                    Statement::Assign(Place::Local(dst), rval, _) => {
                        match rval {
                            Rvalue::Use(op) => {
                                let folded_op = Self::resolve_operand(op, &constants);
                                if let Operand::Constant(c) = &folded_op {
                                    constants.insert(*dst, c.clone());
                                } else {
                                    constants.remove(dst);
                                }
                                if *op != folded_op {
                                    *op = folded_op;
                                    changed = true;
                                }
                            }
                            Rvalue::BinaryOp(op, lhs, rhs) => {
                                let l_res = Self::resolve_operand(lhs, &constants);
                                let r_res = Self::resolve_operand(rhs, &constants);

                                if let (Operand::Constant(l_c), Operand::Constant(r_c)) = (&l_res, &r_res) {
                                    if let Some(folded) = Self::fold_binary(*op, l_c, r_c) {
                                        *rval = Rvalue::Use(Operand::Constant(folded.clone()));
                                        constants.insert(*dst, folded);
                                        changed = true;
                                        continue;
                                    }
                                }

                                if *lhs != l_res {
                                    *lhs = l_res;
                                    changed = true;
                                }
                                if *rhs != r_res {
                                    *rhs = r_res;
                                    changed = true;
                                }
                                constants.remove(dst);
                            }
                            Rvalue::UnaryOp(op, operand) => {
                                let op_res = Self::resolve_operand(operand, &constants);
                                if let Operand::Constant(c) = &op_res {
                                    if let Some(folded) = Self::fold_unary(*op, c) {
                                        *rval = Rvalue::Use(Operand::Constant(folded.clone()));
                                        constants.insert(*dst, folded);
                                        changed = true;
                                        continue;
                                    }
                                }
                                if *operand != op_res {
                                    *operand = op_res;
                                    changed = true;
                                }
                                constants.remove(dst);
                            }
                            _ => {
                                constants.remove(dst);
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Optimize Terminator if condition is constant
            if let Some(term) = &mut block.terminator {
                if let Terminator::Cond { cond, then_block, else_block, span } = term {
                    let cond_res = Self::resolve_operand(cond, &constants);
                    if let Operand::Constant(MirConstant::Bool(b)) = cond_res {
                        let target = if b { *then_block } else { *else_block };
                        *term = Terminator::Goto(target, *span);
                        changed = true;
                    } else if *cond != cond_res {
                        *cond = cond_res;
                        changed = true;
                    }
                }
            }
        }

        changed
    }

    fn resolve_operand(op: &Operand, constants: &BTreeMap<LocalId, MirConstant>) -> Operand {
        match op {
            Operand::Copy(Place::Local(id)) | Operand::Move(Place::Local(id)) => {
                if let Some(c) = constants.get(id) {
                    Operand::Constant(c.clone())
                } else {
                    op.clone()
                }
            }
            _ => op.clone(),
        }
    }

    fn fold_binary(op: MirBinaryOp, lhs: &MirConstant, rhs: &MirConstant) -> Option<MirConstant> {
        match (lhs, rhs) {
            (MirConstant::Int(l), MirConstant::Int(r)) => match op {
                MirBinaryOp::Add => Some(MirConstant::Int(l.wrapping_add(*r))),
                MirBinaryOp::Sub => Some(MirConstant::Int(l.wrapping_sub(*r))),
                MirBinaryOp::Mul => Some(MirConstant::Int(l.wrapping_mul(*r))),
                MirBinaryOp::Div => {
                    if *r != 0 {
                        Some(MirConstant::Int(l.wrapping_div(*r)))
                    } else {
                        None // Do not fold div by zero; preserve runtime trap
                    }
                }
                MirBinaryOp::Mod => {
                    if *r != 0 {
                        Some(MirConstant::Int(l.wrapping_rem(*r)))
                    } else {
                        None
                    }
                }
                MirBinaryOp::Eq => Some(MirConstant::Bool(l == r)),
                MirBinaryOp::NotEq => Some(MirConstant::Bool(l != r)),
                MirBinaryOp::Lt => Some(MirConstant::Bool(l < r)),
                MirBinaryOp::LtEq => Some(MirConstant::Bool(l <= r)),
                MirBinaryOp::Gt => Some(MirConstant::Bool(l > r)),
                MirBinaryOp::GtEq => Some(MirConstant::Bool(l >= r)),
                MirBinaryOp::FloorDiv => {
                    if *r != 0 {
                        if *l == i64::MIN && *r == -1 {
                            Some(MirConstant::Int(i64::MIN))
                        } else {
                            let d = l / r;
                            let rem = l % r;
                            let floor_d = if (rem != 0) && ((rem < 0) != (*r < 0)) { d - 1 } else { d };
                            Some(MirConstant::Int(floor_d))
                        }
                    } else {
                        None
                    }
                }
                MirBinaryOp::Pow => {
                    if *r >= 0 {
                        Some(MirConstant::Int(wrapping_pow_i64(*l, *r as u64)))
                    } else {
                        None
                    }
                }
                MirBinaryOp::BitAnd => Some(MirConstant::Int(l & r)),
                MirBinaryOp::BitOr => Some(MirConstant::Int(l | r)),
                MirBinaryOp::BitXor => Some(MirConstant::Int(l ^ r)),
                MirBinaryOp::Shl => Some(MirConstant::Int(l.wrapping_shl((*r & 63) as u32))),
                MirBinaryOp::Shr => Some(MirConstant::Int(l.wrapping_shr((*r & 63) as u32))),
                _ => None,
            },
            (MirConstant::Bool(l), MirConstant::Bool(r)) => match op {
                MirBinaryOp::Eq => Some(MirConstant::Bool(l == r)),
                MirBinaryOp::NotEq => Some(MirConstant::Bool(l != r)),
                MirBinaryOp::And => Some(MirConstant::Bool(*l && *r)),
                MirBinaryOp::Or => Some(MirConstant::Bool(*l || *r)),
                _ => None,
            },
            _ => None,
        }
    }

    fn fold_unary(op: MirUnaryOp, val: &MirConstant) -> Option<MirConstant> {
        match val {
            MirConstant::Int(i) => match op {
                MirUnaryOp::Neg => Some(MirConstant::Int(i.wrapping_neg())),
                MirUnaryOp::BitNot => Some(MirConstant::Int(!i)),
                _ => None,
            },
            MirConstant::Bool(b) => match op {
                MirUnaryOp::Not => Some(MirConstant::Bool(!b)),
                _ => None,
            },
            _ => None,
        }
    }
}

fn wrapping_pow_i64(mut base: i64, mut exp: u64) -> i64 {
    let mut res: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            res = res.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exp >>= 1;
    }
    res
}
