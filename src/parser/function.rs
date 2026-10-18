//! Function declarations/expressions and parameters: `js_parse_function_decl2` (quickjs.c:37379).

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_function_declaration(&mut self, is_async: bool) -> Result<Rc<Function>, ParseError> {
        let span = self.span();
        self.expect_ident("function")?;
        let is_generator = self.eat_punct(Punct::Star)?;
        let name = if let TokenKind::Ident(name) = &self.current.kind {
            let name = name.clone();
            self.advance()?;
            Some(name)
        } else {
            None
        };
        let params = self.parse_parameters()?;
        let body = FunctionBody::Block(self.parse_block()?);
        Ok(Rc::new(Function {
            name,
            params,
            body,
            is_arrow: false,
            is_async,
            is_generator,
            span,
        }))
    }

    pub(super) fn parse_function_expression(&mut self, is_async: bool) -> Result<Rc<Function>, ParseError> {
        let span = self.span();
        self.expect_ident("function")?;
        let is_generator = self.eat_punct(Punct::Star)?;
        let name = if let TokenKind::Ident(name) = &self.current.kind {
            let name = name.clone();
            self.advance()?;
            Some(name)
        } else {
            None
        };
        let params = self.parse_parameters()?;
        let body = FunctionBody::Block(self.parse_block()?);
        Ok(Rc::new(Function {
            name,
            params,
            body,
            is_arrow: false,
            is_async,
            is_generator,
            span,
        }))
    }

    pub(super) fn parse_parameters(&mut self) -> Result<Vec<Pattern>, ParseError> {
        self.expect_punct(Punct::LParen)?;
        let mut params = Vec::new();
        while !self.current.is_punct(Punct::RParen) {
            if self.eat_punct(Punct::Ellipsis)? {
                let pattern = Pattern::Rest(Box::new(self.parse_binding_pattern()?));
                params.push(pattern);
                break;
            }
            params.push(self.parse_binding_pattern_with_default()?);
            if !self.eat_punct(Punct::Comma)? {
                break;
            }
        }
        self.expect_punct(Punct::RParen)?;
        Ok(params)
    }

    pub(super) fn parse_binding_pattern(&mut self) -> Result<Pattern, ParseError> {
        let pattern = if self.eat_punct(Punct::LBracket)? {
            let mut elements = Vec::new();
            while !self.current.is_punct(Punct::RBracket) {
                if self.eat_punct(Punct::Comma)? {
                    elements.push(None);
                    continue;
                }
                if self.eat_punct(Punct::Ellipsis)? {
                    elements.push(Some(Pattern::Rest(Box::new(self.parse_binding_pattern()?))));
                    break;
                }
                elements.push(Some(self.parse_binding_pattern_with_default()?));
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
            }
            self.expect_punct(Punct::RBracket)?;
            Pattern::Array(elements)
        } else if self.eat_punct(Punct::LBrace)? {
            let mut properties = Vec::new();
            while !self.current.is_punct(Punct::RBrace) {
                if self.eat_punct(Punct::Ellipsis)? {
                    let value = self.parse_binding_pattern()?;
                    properties.push(ObjectPatternProperty {
                        key: PropertyName::Ident("...".into()),
                        value,
                    });
                    self.eat_punct(Punct::Comma)?;
                    continue;
                }
                let key = self.parse_property_name()?;
                let value = if self.eat_punct(Punct::Colon)? {
                    self.parse_binding_pattern()?
                } else {
                    match &key {
                        PropertyName::Ident(name) => Pattern::Ident(name.clone()),
                        _ => return Err(self.error("Invalid shorthand property pattern")),
                    }
                };
                let value = if self.eat_punct(Punct::Assign)? {
                    Pattern::Assign {
                        target: Box::new(value),
                        default: Box::new(self.parse_assignment()?),
                    }
                } else {
                    value
                };
                properties.push(ObjectPatternProperty { key, value });
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
            }
            self.expect_punct(Punct::RBrace)?;
            Pattern::Object(properties)
        } else {
            Pattern::Ident(self.binding_ident()?)
        };
        Ok(pattern)
    }

    pub(super) fn parse_binding_pattern_with_default(&mut self) -> Result<Pattern, ParseError> {
        let pattern = self.parse_binding_pattern()?;
        if self.eat_punct(Punct::Assign)? {
            Ok(Pattern::Assign {
                target: Box::new(pattern),
                default: Box::new(self.parse_assignment()?),
            })
        } else {
            Ok(pattern)
        }
    }
}
