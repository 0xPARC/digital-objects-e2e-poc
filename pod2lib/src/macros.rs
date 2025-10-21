use std::sync::Arc;

use pod2::{
    frontend::MainPodBuilder,
    middleware::{CustomPredicateBatch, CustomPredicateRef},
};

#[macro_export]
macro_rules! set {
    () => ({
        pod2::middleware::containers::Set::new(DEPTH, std::collections::HashSet::new()).unwrap()
    });
    ($($val:expr),* ,) => (
        $crate::set!($($val),*).unwrap()
    );
    ($($val:expr),*) => ({
        let mut set = std::collections::HashSet::new();
        $( set.insert($crate::middleware::Value::from($val)); )*
        pod2::middleware::containers::Set::new(DEPTH, set).unwrap()
    });
}

#[macro_export]
macro_rules! dict {
    ({ }) => (
        pod2::middleware::containers::Dictionary::new(DEPTH, std::collections::HashMap::new()).unwrap()
    );
    ({ $($key:expr => $val:expr),* , }) => (
        $crate::dict!({ $($key => $val),* }).unwrap()
    );
    ({ $($key:expr => $val:expr),* }) => ({
        let mut map = std::collections::HashMap::new();
        $( map.insert(pod2::middleware::Key::from($key.clone()), pod2::middleware::Value::from($val.clone())); )*
        pod2::middleware::containers::Dictionary::new(DEPTH, map).unwrap()
    });
}

#[macro_export]
macro_rules! op {
    (Equal($a:expr, $b:expr)) => {
        pod2::frontend::Operation::eq($a.clone(), $b.clone())
    };
    (HashOf($hash:expr, $a:expr, $b:expr)) => {
        pod2::frontend::Operation::hash_of($hash.clone(), $a.clone(), $b.clone())
    };
    (DictContains($dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_contains($dict.clone(), $key.clone(), $value.clone())
    };
    (DictUpdate($dict:expr, $old_dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_update(
            $dict.clone(),
            $old_dict.clone(),
            $key.clone(),
            $value.clone(),
        )
    };
    (DictInsert($dict:expr, $old_dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_insert(
            $dict.clone(),
            $old_dict.clone(),
            $key.clone(),
            $value.clone(),
        )
    };
    (DictDelete($dict:expr, $old_dict:expr, $key:expr)) => {
        pod2::frontend::Operation::dict_delete($dict.clone(), $old_dict.clone(), $key.clone())
    };
    (SetInsert($set:expr, $old_set:expr, $value:expr)) => {
        pod2::frontend::Operation::set_insert($set.clone(), $old_set.clone(), $value.clone())
    };
    (SetDelete($set:expr, $old_set:expr, $value:expr)) => {
        pod2::frontend::Operation::set_delete($set.clone(), $old_set.clone(), $value.clone())
    };
}

pub fn find_custom_pred_by_name(
    batches: &[Arc<CustomPredicateBatch>],
    name: &str,
) -> Option<CustomPredicateRef> {
    for batch in batches {
        for (index, predicate) in batch.predicates().iter().enumerate() {
            if predicate.name == name {
                return Some(CustomPredicateRef {
                    batch: batch.clone(),
                    index,
                });
            }
        }
    }
    None
}

#[macro_export]
macro_rules! _st_custom_args {
    (process_st, $builder:expr, $input_sts:expr, $st:expr) => {{
        $input_sts.push($st);
    }};
    (process_op, $builder:expr, $input_sts:expr, $pred:ident($($args:expr),+)) => {{
        $input_sts.push($builder.priv_op($crate::op!($pred($($args),+))).unwrap());
    }};

    // Munch native operation
    ($builder:expr, $input_sts:expr, $pred:ident($($args:expr),+)) => {{
        $crate::_st_custom_args!(process_op, $builder, $input_sts, $pred($($args),+));
    }};
    ($builder:expr, $input_sts:expr, $pred:ident($($args:expr),+), $($tail:tt)*) => {{
        $crate::_st_custom_args!(process_op, $builder, $input_sts, $pred($($args),+));
        $crate::_st_custom_args!($builder, $input_sts, $($tail)*)
    }};
    // Munch statement
    ($builder:expr, $input_sts:expr, $st:expr) => {{
        $crate::_st_custom_args!(process_st, $builder, $input_sts, $st);
    }};
    ($builder:expr, $input_sts:expr, $st:expr, $($tail:tt)*) => {{
        $crate::_st_custom_args!(process_st, $builder, $input_sts, $st);
        $crate::_st_custom_args!($builder, $input_sts, $($tail)*)
    }};
}

#[macro_export]
macro_rules! _wildcard_values {
    (process, $custom_pred:expr, $values:expr, $name:ident, $value:expr) => {{
        let name = stringify!($name);
        let predicate = &$custom_pred.batch.predicates()[$custom_pred.index];
        let index = predicate.wildcard_names().iter().position(|wc_name| wc_name == name).expect("valid wildcard name");
        $values.push((index, pod2::middleware::Value::from($value.clone())));
    }};

    ($custom_pred:expr, $values:expr, []) => {{
    }};
    // Munch value
    ($custom_pred:expr, $values:expr, [$name:ident=$value:expr]) => {{
        $crate::_wildcard_values!(process, $custom_pred, $values, $name, $value);
    }};
    ($custom_pred:expr, $values:expr, [$name:ident=$value:expr, $($tail:expr),*]) => {{
        $crate::_wildcard_values!(process, $custom_pred, $values, $name, $value);
        $crate::_wildcard_values!($custom_pred, $values, [$($tail),*]);
    }};
}

#[macro_export]
macro_rules! _st_custom {
    ($builder:expr, $batches:expr, $pub:expr, $pred:ident($($wc_name:ident=$wc_value:expr),*) = ($($sts:tt)*)) => {{
        let custom_pred = $crate::macros::find_custom_pred_by_name($batches, stringify!($pred)).unwrap();
        let mut input_sts = Vec::new();
        $crate::_st_custom_args!($builder, &mut input_sts, $($sts)*);
        let mut wildcard_values: Vec<(usize, pod2::middleware::Value)> = Vec::new();
        $crate::_wildcard_values!(custom_pred, wildcard_values, [$($wc_name=$wc_value),*]);
        let op = pod2::frontend::Operation::custom(custom_pred, input_sts);
        $builder
            .op($pub, wildcard_values, op)
            .unwrap()
    }};
}

pub struct BuildContext<'a> {
    pub builder: &'a mut MainPodBuilder,
    pub batches: &'a [Arc<CustomPredicateBatch>],
}

/// Argument types:
/// Same as `st_custom!`
#[macro_export]
#[rustfmt::skip]
macro_rules! pub_st_custom {
    ($ctx:expr, $pred:ident($($wc_name:ident=$wc_value:expr),*) = ($($sts:tt)*)) => {{
        $crate::_st_custom!($ctx.builder, $ctx.batches, true, $pred($($wc_name=$wc_value),*) = ($($sts)*))
    }};
}

/// Argument types:
/// $ctx: &mut BuildContext
/// $pred: NativePredicate token
/// $wc_name: Public wildcard name token
/// $wc_value: &Into<Value>
/// $sts: Operation|Statement
#[macro_export]
#[rustfmt::skip]
macro_rules! st_custom {
    ($ctx:expr, $pred:ident($($wc_name:ident=$wc_value:expr),*) = ($($sts:tt)*)) => {{
        $crate::_st_custom!($ctx.builder, $ctx.batches, false, $pred($($wc_name=$wc_value),*) = ($($sts)*))
    }};
}
