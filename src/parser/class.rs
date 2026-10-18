//! Class parsing: `js_parse_class` (quickjs.c:25571).

use super::*;

impl<'a> Parser<'a> {
    /// QuickJS `js_parse_class` (quickjs.c:25571).
    pub(super) fn parse_class(&mut self, _declaration: bool) -> Result<Rc<ClassDecl>, ParseError> {
        let span = self.span();
        self.expect_ident("class")?;
        let name = if let TokenKind::Ident(name) = &self.current.kind {
            if !self.current.is_ident("extends") {
                let name = name.clone();
                self.advance()?;
                Some(name)
            } else {
                None
            }
        } else {
            None
        };
        let super_class = if self.eat_ident("extends")? {
            Some(Box::new(self.parse_left_hand_side()?))
        } else {
            None
        };
        self.expect_punct(Punct::LBrace)?;
        let mut elements = Vec::new();
        while !self.current.is_punct(Punct::RBrace) {
            if self.eat_punct(Punct::Semicolon)? {
                continue;
            }
            let mut is_static = false;
            if self.current.is_ident("static") {
                let next = self.peek();
                if next.as_ref().is_some_and(|token| {
                    !token.is_punct(Punct::LParen) && !token.is_punct(Punct::Assign)
                }) {
                    self.advance()?;
                    is_static = true;
                }
            }
            let mut is_async = false;
            if self.current.is_ident("async")
                && let Some(next) = self.peek()
                && (matches!(
                    next.kind,
                    TokenKind::Ident(_) | TokenKind::String(_) | TokenKind::Number(_)
                ) || next.is_punct(Punct::LBracket)
                    || next.is_punct(Punct::Star))
            {
                self.advance()?;
                is_async = true;
            }
            let mut kind = ClassElementKind::Method;
            if self.current.is_ident("get") {
                if let Some(next) = self.peek()
                    && !next.is_punct(Punct::LParen)
                {
                    self.advance()?;
                    kind = ClassElementKind::Getter;
                }
            } else if self.current.is_ident("set")
                && let Some(next) = self.peek()
                && !next.is_punct(Punct::LParen)
            {
                self.advance()?;
                kind = ClassElementKind::Setter;
            }
            let generator = self.eat_punct(Punct::Star)?;
            let key = self.parse_property_name()?;
            if self.current.is_punct(Punct::LParen) {
                let function = self.parse_method(
                    key.clone(),
                    kind == ClassElementKind::Method,
                    generator,
                    is_async,
                )?;
                elements.push(ClassElement {
                    key,
                    kind,
                    is_static,
                    function: Some(function),
                    value: None,
                });
            } else {
                let value = if self.eat_punct(Punct::Assign)? {
                    Some(self.parse_assignment()?)
                } else {
                    None
                };
                self.consume_semicolon()?;
                elements.push(ClassElement {
                    key,
                    kind: ClassElementKind::Field,
                    is_static,
                    function: None,
                    value,
                });
            }
        }
        self.expect_punct(Punct::RBrace)?;
        Ok(Rc::new(ClassDecl {
            name,
            super_class,
            elements,
            span,
        }))
    }

    pub(super) fn parse_method(
        &mut self,
        key: PropertyName,
        is_method: bool,
        generator: bool,
        is_async: bool,
    ) -> Result<Rc<Function>, ParseError> {
        let span = self.span();
        let name = property_name_string(&key);
        let saved = self.yield_allowed;
        self.yield_allowed = generator;
        let params = self.parse_parameters()?;
        let body = FunctionBody::Block(self.parse_block()?);
        self.yield_allowed = saved;
        let _ = is_method;
        Ok(Rc::new(Function {
            name: Some(name),
            params,
            body,
            is_arrow: false,
            is_async,
            is_generator: generator,
            span,
        }))
    }

    pub(super) fn parse_property_name(&mut self) -> Result<PropertyName, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(name) | TokenKind::PrivateIdent(name) => {
                let name = name.clone();
                self.advance()?;
                Ok(PropertyName::Ident(name))
            }
            TokenKind::String(value) => {
                let value = value.clone();
                self.advance()?;
                Ok(PropertyName::String(value))
            }
            TokenKind::Number(value) => {
                let value = *value;
                self.advance()?;
                Ok(PropertyName::Number(value))
            }
            TokenKind::Punct(Punct::LBracket) => {
                self.advance()?;
                let expression = self.parse_expression()?;
                self.expect_punct(Punct::RBracket)?;
                Ok(PropertyName::Computed(Box::new(expression)))
            }
            _ => Err(self.error(format!(
                "Expected property name, found {}",
                self.current.description()
            ))),
        }
    }
}
