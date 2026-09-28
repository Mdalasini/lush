//! Typing environment and prelude / stdlib stubs.

use crate::ty::{Scheme, Type};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    pub(crate) values: HashMap<String, Scheme>,
    pub(crate) types: HashMap<String, usize>,
}

impl Env {
    pub fn prelude() -> Self {
        let mut env = Env::default();
        for (name, arity) in [
            ("Int", 0),
            ("Float", 0),
            ("String", 0),
            ("Bool", 0),
            ("Nil", 0),
            ("List", 1),
            ("Result", 2),
            ("Option", 1),
            ("BitArray", 0),
            ("Pid", 0),
            ("Subject", 1),
            ("Monitor", 0),
            ("Timer", 0),
            ("Selector", 1),
        ] {
            env.types.insert(name.into(), arity);
        }

        env.insert_mono("True", Type::bool());
        env.insert_mono("False", Type::bool());
        env.insert_mono("Nil", Type::nil());
        env.insert_scheme(
            "Ok",
            Scheme {
                vars: vec![0, 1],
                body: Type::Fn {
                    params: vec![Type::Var(0)],
                    ret: Box::new(Type::result(Type::Var(0), Type::Var(1))),
                },
                constraints: vec![],
            },
        );
        env.insert_scheme(
            "Error",
            Scheme {
                vars: vec![0, 1],
                body: Type::Fn {
                    params: vec![Type::Var(1)],
                    ret: Box::new(Type::result(Type::Var(0), Type::Var(1))),
                },
                constraints: vec![],
            },
        );
        env.insert_scheme(
            "Some",
            Scheme {
                vars: vec![0],
                body: Type::Fn {
                    params: vec![Type::Var(0)],
                    ret: Box::new(Type::option(Type::Var(0))),
                },
                constraints: vec![],
            },
        );
        env.insert_scheme(
            "None",
            Scheme {
                vars: vec![0],
                body: Type::option(Type::Var(0)),
                constraints: vec![],
            },
        );
        env
    }

    pub fn insert_mono(&mut self, name: impl Into<String>, ty: Type) {
        self.values.insert(name.into(), Scheme::mono(ty));
    }

    pub fn insert_scheme(&mut self, name: impl Into<String>, scheme: Scheme) {
        self.values.insert(name.into(), scheme);
    }

    pub fn get(&self, name: &str) -> Option<&Scheme> {
        self.values.get(name)
    }

    pub fn extend(&mut self, name: impl Into<String>, scheme: Scheme) {
        self.values.insert(name.into(), scheme);
    }

    pub fn bind_module_stub(&mut self, alias: &str) {
        self.insert_mono(
            alias,
            Type::Named {
                module: None,
                name: format!("Module_{alias}"),
                args: vec![],
            },
        );
    }
}
