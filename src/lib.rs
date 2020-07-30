extern crate redis_module;
extern crate itertools;

use std::any::{Any, type_name};
use std::collections::HashMap;
use std::fmt::Debug;

use dyn_clonable::*;
use redis_module::{RedisError, parse_unsigned_integer, parse_integer, parse_float};
use itertools::Itertools;

#[derive(Debug, PartialEq)]
pub struct Command {
    pub name: &'static str,
    required_args: Vec<Arg>,
    optional_args: Vec<Arg>,
    kwargs: HashMap<&'static str, Arg>,
}

thread_local! {
    static TN_STRING: &'static str = type_name::<String>();
    static TN_U64: &'static str = type_name::<u64>();
    static TN_I64: &'static str = type_name::<i64>();
    static TN_F64: &'static str = type_name::<f64>();
}

macro_rules! parse_arg {
    (
        $arg:ident,
        $next_arg:ident,
        $raw_args:ident
    ) => {
        match $arg.kind {
            Collection::Unit => {
                match $arg.type_name {
                    n if n == TN_STRING.with(|t| t.clone()) => {
                        Box::new($next_arg.clone())
                    },
                    n if n == TN_U64.with(|t| t.clone()) => {
                        Box::new(parse_unsigned_integer($next_arg.as_str())?)
                    },
                    n if n == TN_I64.with(|t| t.clone()) => {
                        Box::new(parse_integer($next_arg.as_str())?)
                    },
                    n if n == TN_F64.with(|t| t.clone()) => {
                        Box::new(parse_float($next_arg.as_str())?)
                    },
                    _ => return Err(RedisError::String(format!("{} is not a supported type", $arg.type_name)))
                }
            },
            Collection::Vec => {
                let len = parse_unsigned_integer($next_arg.as_str())? as usize;
                let mut val: Vec<Box<dyn Value>> = Vec::with_capacity(len);
                for _ in 0..len {
                    match $raw_args.next() {
                        Some(next) => {
                            $next_arg = next;
                        },
                        None => {
                            return Err(RedisError::WrongArity);
                        }
                    };
                    match $arg.type_name {
                        n if n == TN_STRING.with(|t| t.clone()) => {
                            val.push(Box::new($next_arg.clone()));
                        },
                        n if n == TN_U64.with(|t| t.clone()) => {
                            val.push(Box::new(parse_unsigned_integer($next_arg.as_str())?));
                        },
                        n if n == TN_I64.with(|t| t.clone()) => {
                            val.push(Box::new(parse_integer($next_arg.as_str())?));
                        },
                        n if n == TN_F64.with(|t| t.clone()) => {
                            val.push(Box::new(parse_float($next_arg.as_str())?));
                        },
                        _ => return Err(RedisError::String(format!("{} is not a supported type", $arg.type_name)))
                    }
                }
                Box::new(val)
            },
        }
    };
}

impl Command {
    pub fn new(name: &'static str) -> Self{
        Command {name, required_args: Vec::new(), optional_args: Vec::new(), kwargs: HashMap::new()}
    }

    pub fn add_arg(&mut self, arg: Arg) {
        match arg.arg_type {
            ArgType::Arg => {
                if arg.default.is_none() {
                    self.required_args.push(arg);
                } else {
                    self.optional_args.push(arg);
                }
            },
            ArgType::Kwarg => {
                self.kwargs.insert(arg.arg, arg);
            }
        }
    }

    pub fn parse_args(&self, raw_args: Vec<String>) -> Result<HashMap<&'static str, Box<dyn Value>>, RedisError> {
        let mut raw_args = raw_args.into_iter();
        match raw_args.next() {
           Some(cmd_name) => {
               if cmd_name.to_lowercase() != self.name {
                   return Err(RedisError::String(format!("Expected {}, got {}", self.name, cmd_name)))
               }
           },
           None => return Err(RedisError::WrongArity)
        }
        
        let mut res = HashMap::new();

        // parse args
        let mut required_pos: usize = 0;
        let mut optional_pos: usize = 0;
        let mut do_optional = true;
        while let Some(mut next_arg) = raw_args.next() {
            // match required args
            if required_pos < self.required_args.len() {
                let arg = &self.required_args[required_pos];

                let val: Box<dyn Value> = parse_arg!(arg, next_arg, raw_args);
                res.insert(arg.arg, val);
                required_pos += 1;
                
                continue;
            }
            
            if let Some(arg) = self.kwargs.get(next_arg.to_lowercase().as_str()) {
                // if we can match named args, then done with optional
                if do_optional {
                    do_optional = false;
                }

                let val: Box<dyn Value> = match raw_args.next() {
                    Some(mut next) => parse_arg!(arg, next, raw_args),
                    None => return Err(RedisError::WrongArity)
                };
                
                res.insert(arg.arg, val);
            } else {
                // match optional args
                if do_optional && optional_pos < self.optional_args.len() {
                    let arg = &self.optional_args[optional_pos];

                    let val: Box<dyn Value> = parse_arg!(arg, next_arg, raw_args);
                    res.insert(arg.arg, val);
                    optional_pos += 1;
                } else {
                    return Err(RedisError::String(format!("Unexpected arg {}", next_arg)))
                }
            }
        }

        // check if all required args are fulfilled
        for v in self.required_args.iter() {
            if !res.contains_key(v.arg) {
                return Err(RedisError::String(format!("{} is required", v.arg)))
            }
        }

        // check if all optional args are fulfilled
        for v in self.optional_args.iter() {
            if !res.contains_key(v.arg) {
                res.insert(v.arg, v.default.as_ref().unwrap().clone());
            }
        }

        // check if all kwargs are fulfilled
        for (k, v) in self.kwargs.iter() {
            if !res.contains_key(k) {
                if v.default.is_none() {
                    return Err(RedisError::String(format!("{} is required", v.arg)))
                }
                res.insert(k.to_owned(), v.default.as_ref().unwrap().clone());
            }
        }

        Ok(res)
    }
}

#[clonable]
pub trait Value: Any + Debug + Clone {
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    fn as_string(self: Box<Self>) -> Result<String, RedisError>;
    fn as_u64(self: Box<Self>) -> Result<u64, RedisError>;
    fn as_i64(self: Box<Self>) -> Result<i64, RedisError>;
    fn as_f64(self: Box<Self>) -> Result<f64, RedisError>;
    fn as_vec(self: Box<Self>) -> Result<Vec<Box<dyn Value>>, RedisError>;
    fn as_stringvec(self: Box<Self>) -> Result<Vec<String>, RedisError>;
    fn as_u64vec(self: Box<Self>) -> Result<Vec<u64>, RedisError>;
    fn as_i64vec(self: Box<Self>) -> Result<Vec<i64>, RedisError>;
    fn as_f64vec(self: Box<Self>) -> Result<Vec<f64>, RedisError>;
}

impl<T: Any + Debug + Clone > Value for T {
    fn into_any(self: Box<Self>) -> Box<dyn Any> { self }

    fn as_string(self: Box<Self>) -> Result<String, RedisError> {
        match self.into_any().downcast::<String>() {
            Ok(d) => Ok(*d),
            Err(e) => Err(RedisError::String(format!("Unable to cast {:?} into String", e)))
        }
    }

    fn as_u64(self: Box<Self>) -> Result<u64, RedisError> {
        match self.into_any().downcast::<u64>() {
            Ok(d) => Ok(*d),
            Err(e) => Err(RedisError::String(format!("Unable to cast {:?} into u64", e)))
        }
    }

    fn as_i64(self: Box<Self>) -> Result<i64, RedisError> {
        match self.into_any().downcast::<i64>() {
            Ok(d) => Ok(*d),
            Err(e) => Err(RedisError::String(format!("Unable to cast {:?} into i64", e)))
        }
    }

    fn as_f64(self: Box<Self>) -> Result<f64, RedisError> {
        match self.into_any().downcast::<f64>() {
            Ok(d) => Ok(*d),
            Err(e) => Err(RedisError::String(format!("Unable to cast {:?} into f64", e)))
        }
    }

    fn as_vec(self: Box<Self>) -> Result<Vec<Box<dyn Value>>, RedisError> {
        match self.into_any().downcast::<Vec<Box<dyn Value>>>() {
            Ok(d) => Ok(*d),
            Err(e) => Err(RedisError::String(format!("Unable to cast {:?} into String vec", e)))
        }
    }

    fn as_stringvec(self: Box<Self>) -> Result<Vec<String>, RedisError> {
        self.as_vec()?
            .into_iter()
            .map(|x| x.as_string())
            .fold_results(Vec::new(), |mut a, b| {
                a.push(b);
                a
            })
    }

    fn as_u64vec(self: Box<Self>) -> Result<Vec<u64>, RedisError> {
        self.as_vec()?
            .into_iter()
            .map(|x| x.as_u64())
            .fold_results(Vec::new(), |mut a, b| {
                a.push(b);
                a
            })
    }

    fn as_i64vec(self: Box<Self>) -> Result<Vec<i64>, RedisError> {
        self.as_vec()?
            .into_iter()
            .map(|x| x.as_i64())
            .fold_results(Vec::new(), |mut a, b| {
                a.push(b);
                a
            })
    }

    fn as_f64vec(self: Box<Self>) -> Result<Vec<f64>, RedisError> {
        self.as_vec()?
            .into_iter()
            .map(|x| x.as_f64())
            .fold_results(Vec::new(), |mut a, b| {
                a.push(b);
                a
            })
    }
}

#[derive(Debug, PartialEq)]
pub enum ArgType {
    Arg,
    Kwarg,
}

#[derive(Debug, PartialEq)]
pub enum Collection {
    Unit,
    Vec,
}

#[derive(Debug)]
pub struct Arg {
    pub arg: &'static str,
    pub arg_type: ArgType,
    pub type_name: &'static str,
    pub kind: Collection,
    pub default: Option<Box<dyn Value>>,
}

impl Arg {
    pub fn new(arg: &'static str, arg_type: ArgType, type_name: &'static str, kind: Collection, default: Option<Box<dyn Value>>) -> Self {
        Arg {arg, arg_type, type_name, kind, default}
    }
}

impl std::cmp::PartialEq for Arg {
    fn eq(&self, other: &Self) -> bool {
        self.arg == other.arg &&
        self.arg_type == other.arg_type &&
        self.type_name == other.type_name &&
        self.kind == other.kind &&
        self.default.is_none() == other.default.is_none()
    }
}

#[macro_export]
macro_rules! argument {
    ([
        $arg:expr,
        $argtype:expr,
        $type:ty,
        $kind:expr,
        $default:expr
    ]) => {
        $crate::Arg::new($arg, $argtype, std::any::type_name::<$type>(), $kind, $default)
    };
}

#[macro_export]
macro_rules! command {
    (
        name: $name:expr,
        args: [
            $($arg:tt),* $(,)*
        ] $(,)*
    ) => {{
        let mut _cmd = $crate::Command::new($name);
        $(
            let arg = argument!($arg);
            _cmd.add_arg(arg);
        )*
        _cmd
    }};
}

#[cfg(test)]
mod tests {
    use super::{Arg, Command, ArgType, Collection};
    
    extern crate redis_module;

    #[test]
    fn macro_test() {
        let cmd = command!{
            name: "test",
            args: [
                ["stringarg", ArgType::Arg, String, Collection::Unit, None],
                ["uintarg", ArgType::Kwarg, u64, Collection::Unit, Some(Box::new(1_u64))],
                ["intarg", ArgType::Kwarg, i64, Collection::Unit, Some(Box::new(1_i64))],
                ["floatarg", ArgType::Kwarg, f64, Collection::Unit, Some(Box::new(1_f64))],
            ],
        };

        let mut exp = Command::new("test");
        let arg1 = Arg::new("stringarg", ArgType::Arg, std::any::type_name::<String>(), Collection::Unit, None);
        let arg2 = Arg::new("uintarg", ArgType::Kwarg, std::any::type_name::<u64>(), Collection::Unit, Some(Box::new(1_u64)));
        let arg3 = Arg::new("intarg", ArgType::Kwarg, std::any::type_name::<i64>(), Collection::Unit, Some(Box::new(1_i64)));
        let arg4 = Arg::new("floatarg", ArgType::Kwarg, std::any::type_name::<f64>(), Collection::Unit, Some(Box::new(1_f64)));
        exp.add_arg(arg1);
        exp.add_arg(arg2);
        exp.add_arg(arg3);
        exp.add_arg(arg4);

        assert_eq!(cmd, exp);
    }

    #[test]
    fn parse_args_test() {
        let cmd = command!{
            name: "test",
            args: [
                ["required", ArgType::Arg, String, Collection::Unit, None],
                ["optional", ArgType::Arg, String, Collection::Unit, Some(Box::new("foo".to_owned()))],
                ["uintarg", ArgType::Kwarg, u64, Collection::Unit, Some(Box::new(1_u64))],
                ["intarg", ArgType::Kwarg, i64, Collection::Unit, None],
                ["floatarg", ArgType::Kwarg, f64, Collection::Unit, None],
            ],
        };

        let raw_args = vec!["test".to_owned()];
        let parsed = cmd.parse_args(raw_args);
        assert_eq!(parsed.is_err(), true);

        let raw_args = vec![
            "test".to_owned(),
            "bar".to_owned(),
            "intarg".to_owned(),
            "2".to_owned(),
            "floatarg".to_owned(),
            "3.14".to_owned(),
        ];
        let parsed = cmd.parse_args(raw_args);
        assert_eq!(parsed.is_ok(), true);
        assert_eq!(parsed.is_err(), false);
        
        let mut parsed = parsed.unwrap();
        assert_eq!(
            parsed.remove("required").unwrap().as_string().unwrap(),
            "bar".to_owned()
        );
        assert_eq!(
            parsed.remove("optional").unwrap().as_string().unwrap(),
            "foo".to_owned()
        );
        assert_eq!(
            parsed.remove("uintarg").unwrap().as_u64().unwrap(),
            1_u64
        );
        assert_eq!(
            parsed.remove("intarg").unwrap().as_i64().unwrap(),
            2_i64
        );
        assert_eq!(
            parsed.remove("floatarg").unwrap().as_f64().unwrap(),
            3.14
        );
    }

    #[test]
    fn parse_vec_args_test() {
        let cmd = command!{
            name: "test",
            args: [
                ["foo", ArgType::Arg, String, Collection::Unit, None],
                ["vec1", ArgType::Arg, u64, Collection::Vec, None],
                ["vec2", ArgType::Kwarg, i64, Collection::Vec, None],
                ["fizz", ArgType::Kwarg, String, Collection::Unit, None],
            ],
        };

        let raw_args = vec![
            "test".to_owned(),
            "bar".to_owned(),
            "1".to_owned(),
        ];
        let parsed = cmd.parse_args(raw_args);
        assert_eq!(parsed.is_err(), true);

        let raw_args = vec![
            "test".to_owned(),
            "bar".to_owned(),
            "2".to_owned(),
            "1".to_owned(),
            "1".to_owned(),
            "vec2".to_owned(),
            "3".to_owned(),
            "2".to_owned(),
            "2".to_owned(),
            "2".to_owned(),
            "fizz".to_owned(),
            "buzz".to_owned(),
        ];
        let parsed = cmd.parse_args(raw_args);
        assert_eq!(parsed.is_ok(), true);
        assert_eq!(parsed.is_err(), false);
        
        let mut parsed = parsed.unwrap();
        assert_eq!(
            parsed.remove("foo").unwrap().as_string().unwrap(),
            "bar".to_owned()
        );
        assert_eq!(
            parsed.remove("vec1").unwrap().as_u64vec().unwrap(),
            vec![1_u64; 2]
        );
        assert_eq!(
            parsed.remove("vec2").unwrap().as_i64vec().unwrap(),
            vec![2_i64; 3]
        );
        assert_eq!(
            parsed.remove("fizz").unwrap().as_string().unwrap(),
            "buzz".to_owned()
        );
    }
}