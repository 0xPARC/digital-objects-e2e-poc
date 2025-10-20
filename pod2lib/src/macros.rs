use std::sync::Arc;

use pod2::middleware::{CustomPredicateBatch, CustomPredicateRef};

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
    ($builder:expr, $input_sts:expr,) => {{
    }};
    ($builder:expr, $input_sts:expr, $pred:ident($($args:expr),+), $($tail:tt)*) => {{
        $input_sts.push($builder.priv_op($crate::op!($pred($($args),+))).unwrap());
        $crate::_st_custom_args!($builder, $input_sts, $($tail)*)
    }};
    ($builder:expr, $input_sts:expr, $st:expr, $($tail:tt)*) => {{
        $input_sts.push($st);
        $crate::_st_custom_args!($builder, $input_sts, $($tail)*)
    }};
}

#[macro_export]
macro_rules! _wildcard_values {
    ($values:expr, $index:expr, []) => {{}};
    // ($values:expr, $index:expr, _, $($tail:expr),*) => {{
    //     $crate::_wildcard_values!($values, $index+1, $($tail),*);
    // }};
    ($values:expr, $index:expr, [$value:expr]) => {{
        $values.push(($index, pod2::middleware::Value::from($value.clone())));
    }};
    ($values:expr, $index:expr, [$value:expr, $($tail:expr),*]) => {{
        $values.push(($index, pod2::middleware::Value::from($value.clone())));
        $crate::_wildcard_values!($values, $index+1, [$($tail),*]);
    }};
}

#[macro_export]
macro_rules! _st_custom {
    (($builder:expr, $batches:expr), $pub:expr, $pred:ident($($args:expr),*) = ($($sts:tt)*)) => {{
        let custom_pred = $crate::macros::find_custom_pred_by_name($batches, stringify!($pred)).unwrap();
        let mut input_sts = Vec::new();
        $crate::_st_custom_args!($builder, &mut input_sts, $($sts)*);
        let mut wildcard_values: Vec<(usize, pod2::middleware::Value)> = Vec::new();
        $crate::_wildcard_values!(wildcard_values, 0, [$($args),*]);
        let op = pod2::frontend::Operation::custom(custom_pred, input_sts);
        $builder
            .op($pub, wildcard_values, op)
            .unwrap()
    }};
}

/// Argument types:
/// $builder: &mut MainPodBuilder
/// $batches: &[Arc<CustomPredicateBatch]
/// $args: Operation|Statement
#[macro_export]
#[rustfmt::skip]
macro_rules! pub_st_custom {
    (($builder:expr, $batches:expr), $pred:ident($($args:expr),*) = ($($sts:tt)*)) => {{
        $crate::_st_custom!(($builder, $batches), true, $pred($($args),*) = ($($sts)*))
    }};
}

/// Argument types:
/// $builder: &mut MainPodBuilder
/// $batches: &[Arc<CustomPredicateBatch]
/// $args: Operation|Statement
#[macro_export]
#[rustfmt::skip]
macro_rules! st_custom {
    (($builder:expr, $batches:expr), $pred:ident($($args:expr),*) = ($($sts:tt)*)) => {{
        $crate::_st_custom!(($builder, $batches), false, $pred($($args),*) = ($($sts)*))
    }};
}
