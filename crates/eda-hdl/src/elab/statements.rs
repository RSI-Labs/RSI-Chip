//! Procedural statement, lvalue, local constant, and range elaboration.

use super::*;

impl<'a> Elaborator<'a> {
    pub(super) fn elab_stmt(&mut self, s: &ast::Stmt, env: &Env) -> Result<StmtIR, String> {
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            if object == "$cast" && method == "cast" {
                return self.class_cast_effects(arguments, env);
            }
        }
        // Class handles can be explicitly cleared with `h = null;`. As with
        // construction, clearing detaches the handle from its alias group.
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        e: ast::Expr::Ident(rhs),
                        ..
                    } if rhs == "null"
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        e: ast::Expr::Ident(rhs),
                        ..
                    } if rhs == "null"
                )
            };
            if !matches {
                continue;
            }
            let lv = match s {
                ast::Stmt::BL { lv, .. } | ast::Stmt::NBL { lv, .. } => lv,
                _ => unreachable!("guard matched a null assignment"),
            };
            let path: Vec<String> = match lv {
                ast::Expr::Ident(name) => vec![name.clone()],
                ast::Expr::Member(object, field) => vec![object.clone(), field.clone()],
                ast::Expr::MemberChain(path) => path.clone(),
                _ => return Err("null can only be assigned to a class handle".into()),
            };
            match self.resolve_class_path(env, &path)? {
                ClassPathTarget::Handle { flag_net, .. } => {
                    return self.class_null_statement(&flag_net, nonblocking, env);
                }
                ClassPathTarget::Field(_) => {
                    return Err("null can only be assigned to a class handle".to_string());
                }
            }
        }
        // Class shallow-copy construction: `dst = new source;` (without the
        // parentheses used for an ordinary constructor call).
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        lv: ast::Expr::Ident(_),
                        e: ast::Expr::New(_),
                        ..
                    }
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        lv: ast::Expr::Ident(_),
                        e: ast::Expr::New(_),
                        ..
                    }
                )
            };
            if !matches {
                continue;
            }
            let (destination, source) = match s {
                ast::Stmt::BL {
                    lv: ast::Expr::Ident(destination),
                    e: ast::Expr::New(source),
                }
                | ast::Stmt::NBL {
                    lv: ast::Expr::Ident(destination),
                    e: ast::Expr::New(source),
                } => (destination, source),
                _ => unreachable!("guard matched class shallow copy"),
            };
            if let ast::Expr::Ident(source_handle) = &**source {
                if env.class_handles.contains_key(destination)
                    && (env.class_handles.contains_key(source_handle)
                        || env.class_aliases.borrow().contains_key(source_handle))
                {
                    return self.class_shallow_copy_statement(
                        destination,
                        source_handle,
                        nonblocking,
                        env,
                    );
                }
            }
        }
        // Nested class construction: `this.d = new(...)` or `h.d = new(...)`.
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        lv: ast::Expr::MemberChain(_),
                        e: ast::Expr::NewClass(_),
                    } | ast::Stmt::NBL {
                        lv: ast::Expr::Member(_, _),
                        e: ast::Expr::NewClass(_),
                    }
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        lv: ast::Expr::MemberChain(_),
                        e: ast::Expr::NewClass(_),
                    } | ast::Stmt::BL {
                        lv: ast::Expr::Member(_, _),
                        e: ast::Expr::NewClass(_),
                    }
                )
            };
            if matches {
                let (lv, actuals) = match s {
                    ast::Stmt::BL { lv, e } | ast::Stmt::NBL { lv, e } => match e {
                        ast::Expr::NewClass(actuals) => (lv, actuals),
                        _ => unreachable!("guard matched NewClass"),
                    },
                    _ => unreachable!("guard matched BL/NBL"),
                };
                let path: Vec<String> = match lv {
                    ast::Expr::MemberChain(path) => path.clone(),
                    ast::Expr::Member(object, field) => vec![object.clone(), field.clone()],
                    _ => unreachable!("guard matched member lvalue"),
                };
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle {
                        flag_net,
                        prefix,
                        class,
                    } => {
                        return self.class_new_statement(
                            &flag_net,
                            &prefix,
                            &class,
                            actuals,
                            nonblocking,
                            env,
                        );
                    }
                    ClassPathTarget::Field(_) => {
                        return Err("new can only construct a class-typed property".to_string());
                    }
                }
            }
        }
        // 嵌套类属性 = 类句柄实参：引用别名（如 this.sub = s）。
        if let ast::Stmt::BL {
            lv,
            e: ast::Expr::Ident(src),
        } = s
        {
            let src_is_handle = src == "this"
                || src == "super"
                || env.class_handles.contains_key(src)
                || env.class_aliases.borrow().contains_key(src);
            if src_is_handle {
                let path: Vec<String> = match lv {
                    ast::Expr::Member(o, f) => vec![o.clone(), f.clone()],
                    ast::Expr::MemberChain(p) => p.clone(),
                    _ => Vec::new(),
                };
                if path.len() >= 2 {
                    if let Ok(ClassPathTarget::Handle { prefix, .. }) =
                        self.resolve_class_path(env, &path)
                    {
                        let src_canon = if src == "this" || src == "super" {
                            env.this_handle
                                .map(|(handle, _)| env.canon(handle))
                                .ok_or_else(|| {
                                    format!("'{src}' is valid only inside a class method")
                                })?
                        } else {
                            env.canon(src)
                        };
                        let dst_canon = env.canon(&prefix);
                        let mut aliases = env.class_aliases.borrow_mut();
                        let members: Vec<String> = aliases
                            .iter()
                            .filter(|(_, c)| **c == dst_canon)
                            .map(|(h, _)| h.clone())
                            .collect();
                        for member in members {
                            aliases.insert(member, src_canon.clone());
                        }
                        aliases.insert(prefix, src_canon);
                        return Ok(StmtIR::Block(Vec::new()));
                    }
                }
            }
        }
        // Chained method call statement: `this.d.m(...)` / `h.d.m(...)`.
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            let path: Vec<String> = object.split('.').map(str::to_string).collect();
            if path.len() >= 2
                && (path[0] == "this"
                    || path[0] == "super"
                    || env.class_handles.contains_key(&path[0])
                    || self.classes.contains_key(&path[0]))
            {
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle { prefix, class, .. } => {
                        return self.class_method_statement_on(
                            prefix.as_str(),
                            class.as_str(),
                            class.as_str(),
                            false,
                            method,
                            arguments,
                            env,
                        );
                    }
                    ClassPathTarget::Field(net) => {
                        let entry = env.net(&net)?.clone();
                        if method == "putc" && entry.type_name.as_deref() == Some("string") {
                            return self.string_putc_statement(&entry, arguments, env);
                        }
                        if entry.type_name.as_deref() == Some("string")
                            && matches!(method.as_str(), "itoa" | "hextoa" | "bintoa")
                        {
                            return if method == "itoa" {
                                self.string_itoa_statement(&entry, arguments, env)
                            } else {
                                self.string_power_radix_statement(&entry, method, arguments, env)
                            };
                        }
                        return Err(format!(
                            "method '{method}' called on a non-object field path"
                        ));
                    }
                }
            }
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::NewClass(actuals),
        } = s
        {
            let class = env
                .class_handles
                .get(name)
                .cloned()
                .ok_or_else(|| format!("'{name}' is not a class handle"))?;
            return self.class_new_statement(name, name, &class, actuals, false, env);
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::NewClass(actuals),
        } = s
        {
            let class = env
                .class_handles
                .get(name)
                .cloned()
                .ok_or_else(|| format!("'{name}' is not a class handle"))?;
            return self.class_new_statement(name, name, &class, actuals, true, env);
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(dst),
            e: ast::Expr::Ident(src),
        } = s
        {
            if env.class_handles.contains_key(dst) && env.class_handles.contains_key(src) {
                return self.class_handle_copy(dst, src, false, env);
            }
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Ident(dst),
            e: ast::Expr::Ident(src),
        } = s
        {
            if env.class_handles.contains_key(dst) && env.class_handles.contains_key(src) {
                return self.class_handle_copy(dst, src, true, env);
            }
        }
        // Direct assignment from a class method is the one expression-call
        // context that can safely lower method side effects before reading a
        // shared result scratch net.
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        e: ast::Expr::MethodCall { .. },
                        ..
                    } | ast::Stmt::NBL {
                        e: ast::Expr::MethodCallChain { .. },
                        ..
                    }
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        e: ast::Expr::MethodCall { .. },
                        ..
                    } | ast::Stmt::BL {
                        e: ast::Expr::MethodCallChain { .. },
                        ..
                    }
                )
            };
            let matches = matches || is_cast_assignment(s);
            if !matches {
                continue;
            }
            let (lv, call) = match s {
                ast::Stmt::BL { lv, e } | ast::Stmt::NBL { lv, e } => (lv, e),
                _ => unreachable!("guard matched a class method assignment"),
            };
            if let ast::Expr::Call(name, arguments) = call {
                if name == "$cast" {
                    let target = self.elab_lvalue(lv, env)?;
                    return self.class_cast_assignment(target, arguments, nonblocking, env);
                }
            }
            if let ast::Expr::Ident(name) = lv {
                if let Some((method, base, width)) = env.method_return {
                    if method == name {
                        let value = self.elab_expr(call, env)?;
                        let value = self.resize_class_arg(value, width);
                        return Ok(StmtIR::Bl {
                            lv: LValue {
                                net: base,
                                decl_width: width,
                                range: full_range(width),
                            },
                            e: value,
                        });
                    }
                }
            }
            match call {
                ast::Expr::MethodCall {
                    object,
                    method,
                    arguments,
                } => {
                    if matches!(method.as_str(), "atoi" | "atohex" | "atobin") && !nonblocking {
                        if let Ok(entry) = env.net(object) {
                            if entry.type_name.as_deref() == Some("string") {
                                let target = self.elab_lvalue(lv, env)?;
                                let entry = entry.clone();
                                return self.string_radix_statement(
                                    &entry, target, method, arguments, env,
                                );
                            }
                        }
                    }
                    if self.classes.contains_key(object) {
                        return self.class_static_method_value_assignment(
                            self.elab_lvalue(lv, env)?,
                            object,
                            method,
                            arguments,
                            nonblocking,
                            env,
                        );
                    }
                    if object == "this"
                        || object == "super"
                        || env.class_handles.contains_key(object)
                    {
                        let (handle, class, owner_hint) = if object == "this" || object == "super" {
                            let (handle, class) = env.this_handle.ok_or_else(|| {
                                format!("'{object}' is valid only inside a class method")
                            })?;
                            let start = if object == "super" {
                                env.method_owner
                                    .and_then(|owner| self.classes.get(owner))
                                    .and_then(|definition| definition.extends.clone())
                                    .ok_or_else(|| "super call has no base class".to_string())?
                            } else {
                                class.to_string()
                            };
                            (handle.to_string(), class.to_string(), start)
                        } else {
                            let class = env
                                .class_handles
                                .get(object)
                                .cloned()
                                .ok_or_else(|| format!("'{object}' is not a class handle"))?;
                            (object.clone(), class.clone(), class)
                        };
                        return self.class_method_value_assignment(
                            lv,
                            handle.as_str(),
                            class.as_str(),
                            owner_hint.as_str(),
                            object == "super",
                            method,
                            arguments,
                            nonblocking,
                            env,
                        );
                    }
                }
                ast::Expr::MethodCallChain {
                    path,
                    method,
                    arguments,
                } => {
                    if path[0] == "this"
                        || path[0] == "super"
                        || env.class_handles.contains_key(&path[0])
                    {
                        match self.resolve_class_path(env, path)? {
                            ClassPathTarget::Handle { prefix, class, .. } => {
                                return self.class_method_value_assignment(
                                    lv,
                                    prefix.as_str(),
                                    class.as_str(),
                                    class.as_str(),
                                    false,
                                    method,
                                    arguments,
                                    nonblocking,
                                    env,
                                );
                            }
                            ClassPathTarget::Field(net) => {
                                if matches!(method.as_str(), "atoi" | "atohex" | "atobin")
                                    && !nonblocking
                                {
                                    if let Ok(entry) = env.net(&net) {
                                        if entry.type_name.as_deref() == Some("string") {
                                            let target = self.elab_lvalue(lv, env)?;
                                            let entry = entry.clone();
                                            return self.string_radix_statement(
                                                &entry, target, method, arguments, env,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => unreachable!("guard matched a class method assignment"),
            }
        }
        // Return-value assignment inside a statement-invoked method.
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(name),
            e,
        } = s
        {
            if let Some((method, base, width)) = env.method_return {
                if method == name {
                    let value = self.elab_expr(e, env)?;
                    let value = self.resize_class_arg(value, width);
                    return Ok(StmtIR::Bl {
                        lv: LValue {
                            net: base,
                            decl_width: width,
                            range: full_range(width),
                        },
                        e: value,
                    });
                }
            }
        }
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            if object == "this" || object == "super" || env.class_handles.contains_key(object) {
                return self.class_method_statement(object, method, arguments, env);
            }
            if self.classes.contains_key(object) {
                return self.class_static_method_statement(object, method, arguments, env);
            }
            if method == "putc" {
                if let Ok(entry) = env.net(object) {
                    if entry.type_name.as_deref() == Some("string") {
                        let entry = entry.clone();
                        return self.string_putc_statement(&entry, arguments, env);
                    }
                }
            }
            if matches!(method.as_str(), "itoa" | "hextoa" | "bintoa") {
                if let Ok(entry) = env.net(object) {
                    if entry.type_name.as_deref() == Some("string") {
                        let entry = entry.clone();
                        return if method == "itoa" {
                            self.string_itoa_statement(&entry, arguments, env)
                        } else {
                            self.string_power_radix_statement(&entry, method, arguments, env)
                        };
                    }
                }
            }
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::New(size),
        } = s
        {
            return self.dynamic_array_new(name, size, false, env);
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::New(size),
        } = s
        {
            return self.dynamic_array_new(name, size, true, env);
        }
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            return self.dynamic_array_method(object, method, arguments, env);
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Select(name, index),
            e,
        } = s
        {
            if env
                .net(name)
                .map(|entry| entry.array.is_some())
                .unwrap_or(false)
                && self.const_eval(index, env.params, env.module_name).is_err()
            {
                let element_width = env
                    .net(name)?
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
                let transformed = ast::Stmt::BL {
                    lv: ast::Expr::ArrayIndexedPart {
                        name: name.clone(),
                        indexes: vec![index.as_ref().clone()],
                        start: Box::new(constant_expr(0)),
                        width: Box::new(constant_expr(element_width as i64)),
                        ascending: true,
                    },
                    e: e.clone(),
                };
                return self.elab_stmt(&transformed, env);
            }
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Select(name, index),
            e,
        } = s
        {
            if env
                .net(name)
                .map(|entry| entry.array.is_some())
                .unwrap_or(false)
                && self.const_eval(index, env.params, env.module_name).is_err()
            {
                let element_width = env
                    .net(name)?
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
                let transformed = ast::Stmt::NBL {
                    lv: ast::Expr::ArrayIndexedPart {
                        name: name.clone(),
                        indexes: vec![index.as_ref().clone()],
                        start: Box::new(constant_expr(0)),
                        width: Box::new(constant_expr(element_width as i64)),
                        ascending: true,
                    },
                    e: e.clone(),
                };
                return self.elab_stmt(&transformed, env);
            }
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::ArraySelect(name, indexes),
            e,
        } = s
        {
            if indexes
                .iter()
                .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
            {
                let element_width = env
                    .net(name)?
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
                let transformed = ast::Stmt::BL {
                    lv: ast::Expr::ArrayIndexedPart {
                        name: name.clone(),
                        indexes: indexes.clone(),
                        start: Box::new(constant_expr(0)),
                        width: Box::new(constant_expr(element_width as i64)),
                        ascending: true,
                    },
                    e: e.clone(),
                };
                return self.elab_stmt(&transformed, env);
            }
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::ArraySelect(name, indexes),
            e,
        } = s
        {
            if indexes
                .iter()
                .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
            {
                let element_width = env
                    .net(name)?
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
                let transformed = ast::Stmt::NBL {
                    lv: ast::Expr::ArrayIndexedPart {
                        name: name.clone(),
                        indexes: indexes.clone(),
                        start: Box::new(constant_expr(0)),
                        width: Box::new(constant_expr(element_width as i64)),
                        ascending: true,
                    },
                    e: e.clone(),
                };
                return self.elab_stmt(&transformed, env);
            }
        }
        if let ast::Stmt::BL {
            lv:
                ast::Expr::ArrayIndexedPart {
                    name,
                    indexes,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            return self.array_element_indexed_statement(
                name, indexes, start, width, *ascending, e, false, env,
            );
        }
        if let ast::Stmt::NBL {
            lv:
                ast::Expr::ArrayIndexedPart {
                    name,
                    indexes,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            return self.array_element_indexed_statement(
                name, indexes, start, width, *ascending, e, true, env,
            );
        }
        if let ast::Stmt::BL {
            lv:
                ast::Expr::IndexedPart {
                    name,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            if let Ok(entry) = env.net(name) {
                if entry.array.is_none() && entry.msb < entry.lsb {
                    return self
                        .dynamic_indexed_part_bl_reversed(entry, start, width, *ascending, e, env);
                }
            }
            if self.const_eval(start, env.params, env.module_name).is_err() {
                let expanded = self.expand_runtime_indexed_part_assignment(
                    name, start, width, *ascending, e, env,
                )?;
                return self.elab_stmt(&expanded, env);
            }
        }
        if let ast::Stmt::NBL {
            lv:
                ast::Expr::IndexedPart {
                    name,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            if let Ok(entry) = env.net(name) {
                if entry.array.is_none() && entry.msb < entry.lsb {
                    return self.dynamic_indexed_part_nbl(name, start, width, *ascending, e, env);
                }
            }
            if self.const_eval(start, env.params, env.module_name).is_err() {
                return self.dynamic_indexed_part_nbl(name, start, width, *ascending, e, env);
            }
        }
        Ok(match s {
            ast::Stmt::Block(v) => StmtIR::Block(
                v.iter()
                    .map(|x| self.elab_stmt(x, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Stmt::Local { .. } => {
                return Err(
                    "local variable declarations are supported only inside functions".into(),
                )
            }
            ast::Stmt::Assert { cond, message } => StmtIR::Assert {
                cond: self.elab_expr(cond, env)?,
                message: message.clone(),
            },
            ast::Stmt::MethodCall { .. } => {
                return Err("dynamic-array methods are supported only in module processes".into());
            }
            ast::Stmt::For { .. } => {
                return Err("for loops are supported only inside functions".to_string());
            }
            ast::Stmt::Repeat { .. } => {
                return Err("repeat loops are supported only inside functions".to_string());
            }
            ast::Stmt::While { .. } => {
                return Err("while loops are supported only inside functions".to_string());
            }
            ast::Stmt::Forever(b) => StmtIR::Forever(Box::new(self.elab_stmt(b, env)?)),
            ast::Stmt::NBL { lv, e } => StmtIR::NBl {
                lv: self.elab_lvalue(lv, env)?,
                e: self.elab_expr(e, env)?,
            },
            ast::Stmt::BL { lv, e } => StmtIR::Bl {
                lv: self.elab_lvalue(lv, env)?,
                e: self.elab_expr(e, env)?,
            },
            ast::Stmt::If { c, t, f } => StmtIR::If {
                c: self.elab_expr(c, env)?,
                t: Box::new(self.elab_stmt(t, env)?),
                f: match f {
                    Some(x) => Some(Box::new(self.elab_stmt(x, env)?)),
                    None => None,
                },
            },
            ast::Stmt::Case {
                e,
                arms,
                dflt,
                kind,
            } => {
                if *kind != ast::CaseKind::Exact {
                    let mut flattened_arms = Vec::new();
                    for (labels, body) in arms {
                        let body = self.elab_stmt(body, env)?;
                        for label in labels {
                            flattened_arms.push((self.elab_expr(label, env)?, body.clone()));
                        }
                    }
                    let default = dflt
                        .as_ref()
                        .map(|stmt| self.elab_stmt(stmt, env))
                        .transpose()?;
                    let selector = self.elab_expr(e, env)?;
                    return Ok(wildcard_case_as_if(
                        selector,
                        flattened_arms,
                        default,
                        *kind,
                    ));
                }
                let mut out = Vec::new();
                for (labels, body) in arms {
                    let b = self.elab_stmt(body, env)?;
                    for l in labels {
                        out.push((self.elab_expr(l, env)?, b.clone()));
                    }
                }
                StmtIR::Case {
                    e: self.elab_expr(e, env)?,
                    arms: out,
                    dflt: match dflt {
                        Some(x) => Some(Box::new(self.elab_stmt(x, env)?)),
                        None => None,
                    },
                }
            }
            ast::Stmt::Delay { ns, body } => StmtIR::Delay {
                ns: *ns,
                body: Box::new(self.elab_stmt(body, env)?),
            },
            ast::Stmt::Display { fmt, args, monitor } => {
                let mut parts = Vec::new();
                let mut args_ir = Vec::new();
                for part in fmt {
                    match part {
                        ast::DispPartSrc::Str(s) => parts.push(DispPartIR::Str(s.clone())),
                        ast::DispPartSrc::Time => parts.push(DispPartIR::Time),
                        ast::DispPartSrc::Arg(spec) => {
                            let argument = self.elab_expr(&args[args_ir.len()], env)?;
                            args_ir.push(argument);
                            parts.push(DispPartIR::Arg(args_ir.len() - 1, *spec));
                        }
                    }
                }
                StmtIR::Display {
                    parts,
                    args: args_ir,
                    monitor: *monitor,
                }
            }
            ast::Stmt::Finish => StmtIR::Finish,
        })
    }

    pub(super) fn elab_lvalue(&self, e: &ast::Expr, env: &Env) -> Result<LValue, String> {
        match e {
            ast::Expr::Ident(n) => {
                let entry = env.net(n)?;
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                })
            }
            ast::Expr::Select(n, idx) => {
                let entry = env.net(n)?;
                let i = self.const_eval(idx, env.params, env.module_name)?;
                let off = if entry.array.is_some() {
                    entry.array_offset(&[i])?
                } else {
                    entry.offset(i)?
                };
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: if entry.array.is_some() {
                        Range::Span {
                            msb: off
                                + entry
                                    .array
                                    .as_ref()
                                    .map(|array| array.element_width)
                                    .unwrap_or(1)
                                - 1,
                            lsb: off,
                        }
                    } else {
                        Range::Bit(off)
                    },
                })
            }
            ast::Expr::ArraySelect(n, indexes) => {
                let entry = env.net(n)?;
                let mut resolved = Vec::with_capacity(indexes.len());
                for index in indexes {
                    resolved.push(self.const_eval(index, env.params, env.module_name)?);
                }
                let off = entry.array_offset(&resolved)?;
                let element_width = entry
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{n}' is not an unpacked array"))?;
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: off + element_width - 1,
                        lsb: off,
                    },
                })
            }
            ast::Expr::Member(object, field) => {
                let (net, decl_width, range) = self.member_reference(env, object, field, true)?;
                Ok(LValue {
                    net,
                    decl_width,
                    range,
                })
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Field(net) => {
                    let entry = env.net(&net)?;
                    Ok(LValue {
                        net: entry.base,
                        decl_width: entry.width,
                        range: full_range(entry.width),
                    })
                }
                ClassPathTarget::Handle { .. } => {
                    Err("class handle paths cannot be assigned directly; construct with new".into())
                }
            },
            ast::Expr::Part(n, a, b) => {
                let entry = env.net(n)?;
                let ia = self.const_eval(a, env.params, env.module_name)?;
                let ib = self.const_eval(b, env.params, env.module_name)?;
                if entry.array.is_some() {
                    let (lsb, width) = entry.array_slice(ia, ib)?;
                    return Ok(LValue {
                        net: entry.base,
                        decl_width: entry.width,
                        range: Range::Span {
                            msb: lsb + width - 1,
                            lsb,
                        },
                    });
                }
                let (hi, lo) = if ia >= ib { (ia, ib) } else { (ib, ia) };
                let o1 = entry.offset(hi)?;
                let o2 = entry.offset(lo)?;
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: o1.max(o2),
                        lsb: o1.min(o2),
                    },
                })
            }
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => {
                let (msb, lsb) = self.indexed_part_range(name, start, width, *ascending, env)?;
                Ok(LValue {
                    net: env.net(name)?.base,
                    decl_width: env.net(name)?.width,
                    range: Range::Span { msb, lsb },
                })
            }
            other => Err(format!("invalid assignment target: {:?}", other)),
        }
    }

    pub(super) fn const_eval(
        &self,
        expression: &ast::Expr,
        params: &HashMap<String, i64>,
        module_name: &str,
    ) -> Result<i64, String> {
        const_eval_packages(expression, params, module_name, self.packages)
    }

    pub(super) fn ast_range_width(
        &self,
        range: &ast::RangeAst,
        env: &Env<'_>,
    ) -> Result<usize, String> {
        let width = match range {
            Some((msb, lsb)) => {
                let hi = self.const_eval(msb, env.params, env.module_name)?;
                let lo = self.const_eval(lsb, env.params, env.module_name)?;
                (hi - lo).unsigned_abs() as usize + 1
            }
            None => 1,
        };
        if width == 0 || width > 4096 {
            Err(format!("invalid declaration width {}", width))
        } else {
            Ok(width)
        }
    }

    pub(super) fn ast_range_bounds(
        &self,
        range: &ast::RangeAst,
        env: &Env<'_>,
    ) -> Result<(i64, i64), String> {
        match range {
            Some((msb, lsb)) => {
                let hi = self.const_eval(msb, env.params, env.module_name)?;
                let lo = self.const_eval(lsb, env.params, env.module_name)?;
                Ok((hi, lo))
            }
            None => Ok((0, 0)),
        }
    }
}
