pub trait Plugin: Send + Sync {
    fn new() -> Self
    where
        Self: Sized;
}
