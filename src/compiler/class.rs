//! Class declaration compilation, mirroring js_parse_class`n//! (quickjs.c:25571).

use super::*;

impl Compiler {
    pub(super) fn compile_class(&mut self, class: &ClassDecl) -> Result<(), ParseError> {
        if class.super_class.is_some() {
            // The prototype chain and parent constructor are wired at runtime.
        }
        let name = class.name.clone().unwrap_or_else(|| "<anonymous>".into());
        let mut constructor = None;
        let mut methods = Vec::new();
        let mut instance_fields = Vec::new();
        let mut static_fields = Vec::new();
        for element in &class.elements {
            if matches!(element.kind, ClassElementKind::Field) {
                let field_name = static_property_name(&element.key)
                    .ok_or_else(|| self.error("Computed class fields are not supported"))?;
                if element.is_static {
                    static_fields.push((field_name, element.value.clone()));
                } else {
                    instance_fields.push((field_name, element.value.clone()));
                }
                continue;
            }
            let Some(function) = &element.function else {
                continue;
            };
            let method_name = match &element.key {
                PropertyName::Ident(value) | PropertyName::String(value) => value.clone(),
                PropertyName::Number(value) => value.to_string(),
                PropertyName::Computed(_) => {
                    return Err(self.error("Computed class method names are not supported"));
                }
            };
            let is_constructor = !element.is_static && element.kind_is_constructor(&method_name);
            let compiled = self.compile_child_function(
                function,
                is_constructor,
                class.super_class.is_some(),
                false,
            )?;
            if is_constructor {
                constructor = Some(compiled);
            } else {
                methods.push(ClassMethod {
                    name: Rc::from(method_name),
                    function: compiled,
                    is_static: element.is_static,
                    is_getter: matches!(element.kind, ClassElementKind::Getter),
                    is_setter: matches!(element.kind, ClassElementKind::Setter),
                });
            }
        }
        if constructor.is_none() {
            let mut code = Vec::new();
            if class.super_class.is_some() {
                code.push(Op::SuperCall(0));
            }
            code.push(Op::Undefined);
            code.push(Op::Return);
            constructor = Some(Rc::new(FunctionDef {
                name: Rc::from(name.as_str()),
                positions: vec![class.span.start as u32; code.len()],
                code,
                constants: Vec::new(),
                prop_caches: Vec::new(),
                env_size: 0,
                use_env: false,
                length: 0,
                is_arrow: false,
                is_async: false,
                is_generator: false,
                strict: true,
                is_class_fields: false,
                simple_params: 0,
                source: self.source_name.clone(),
                name_atom: std::cell::RefCell::new(None),
            }));
        }
        let instance_init = self.compile_field_initializer(&instance_fields)?;
        let static_init = self.compile_field_initializer(&static_fields)?;
        if instance_init.as_ref().is_some_and(|def| def.use_env)
            || static_init.as_ref().is_some_and(|def| def.use_env)
        {
            self.needs_env_link = true;
        }
        let def = Rc::new(ClassDef {
            name: Rc::from(name.as_str()),
            constructor,
            methods,
            instance_init,
            static_init,
            has_super: class.super_class.is_some(),
        });
        if let Some(parent) = &class.super_class {
            self.compile_expr(parent)?;
        } else {
            self.emit(Op::Undefined);
        }
        let index = self.add_constant(Constant::Class(def));
        self.emit(Op::CreateClass(index));
        Ok(())
    }

    pub(super) fn compile_field_initializer(
        &mut self,
        fields: &[(String, Option<Expr>)],
    ) -> Result<Option<Rc<FunctionDef>>, ParseError> {
        if fields.is_empty() {
            return Ok(None);
        }
        let mut child = Compiler::new(false, self.capture_frames(), self.source_name.clone());
        child.strict = self.strict;
        for (name, initializer) in fields {
            child.emit(Op::This);
            child.push_string(name);
            if let Some(initializer) = initializer {
                child.compile_expr(initializer)?;
            } else {
                child.emit(Op::Undefined);
            }
            child.emit(Op::SetProp);
            child.emit(Op::Pop);
        }
        child.emit(Op::ReturnUndefined);
        Ok(Some(Rc::new(child.finish(
            "<class_fields>",
            0,
            false,
            false,
            false,
            true,
        ))))
    }
}

impl ClassElement {
    pub(super) fn kind_is_constructor(&self, name: &str) -> bool {
        name == "constructor"
    }
}
