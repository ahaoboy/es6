//! Destructuring binding compilation, mirroring the js_parse_destructuring_element
//! emission of QuickJS (quickjs.c:26619).

use super::*;

impl Compiler {
    pub(super) fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        mode: BindingMode,
    ) -> Result<(), ParseError> {
        match pattern {
            Pattern::Ident(name) => {
                if matches!(mode, BindingMode::Global) {
                    let name = self.string_constant(name);
                    self.emit(Op::SetGlobal(name));
                } else {
                    self.store_ident(name, mode);
                }
                Ok(())
            }
            Pattern::Assign { target, default } => {
                self.emit(Op::Dup);
                let jump = self.emit_jump(Op::JumpIfNotUndefined(0));
                self.emit(Op::Pop);
                self.compile_expr(default)?;
                let end = self.position();
                self.patch_jump(jump, end);
                self.bind_pattern(target, mode)
            }
            Pattern::Array(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    let Some(element) = element else {
                        continue;
                    };
                    if let Pattern::Rest(inner) = element {
                        self.emit(Op::ArrayRest(index as u16));
                        self.bind_pattern(inner, mode)?;
                        continue;
                    }
                    self.emit(Op::Dup);
                    self.push_string(&index.to_string());
                    self.emit(Op::GetProp);
                    self.bind_pattern(element, mode)?;
                }
                self.emit(Op::Pop);
                Ok(())
            }
            Pattern::Object(properties) => {
                let mut excluded: Vec<Rc<str>> = Vec::new();
                for property in properties {
                    if let PropertyName::Ident(name) = &property.key
                        && name == "..."
                    {
                        let excluded: Rc<[Rc<str>]> = Rc::from(excluded.clone());
                        let index = self.add_constant(Constant::Strings(excluded));
                        self.emit(Op::ObjectRest(index));
                        self.bind_pattern(&property.value, mode)?;
                        continue;
                    }
                    if let Some(name) = static_property_name(&property.key) {
                        excluded.push(Rc::from(name));
                    }
                    self.emit(Op::Dup);
                    self.compile_key_name(&property.key)?;
                    self.emit(Op::GetProp);
                    self.bind_pattern(&property.value, mode)?;
                }
                self.emit(Op::Pop);
                Ok(())
            }
            Pattern::Rest(inner) => self.bind_pattern(inner, mode),
        }
    }
}
