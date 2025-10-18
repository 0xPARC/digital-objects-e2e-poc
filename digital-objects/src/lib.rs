#[allow(dead_code)]
fn hello() {
    println!("hello world");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello() {
        hello()
    }
}
