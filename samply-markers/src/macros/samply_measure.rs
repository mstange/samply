//! This module defines the [`samply_measure!`](crate::samply_measure) macro for measuring code blocks.

/// Measures the runtime of a block of code by emitting an interval marker.
///
/// * The interval marker's start time is the beginning of the block.
/// * The interval marker's end time is the end of the block.
///
/// The block's returned value is preserved by the macro.
///
/// Supports both synchronous and asynchronous blocks.
///
/// # Examples
///
/// #### Synchronous
/// ```rust
/// # use samply_markers::samply_measure;
/// # struct World { entities: Vec<i32> }
/// # impl World {
/// #     fn new(len: usize) -> Self { Self { entities: vec![1; len] } }
/// #     fn simulate_physics(&mut self) {}
/// #     fn apply_effects(&mut self) {}
/// # }
/// // Measure the duration of update_world every time it is called.
/// fn update_world(world: &mut World) -> usize {
///     samply_measure!({
///         world.simulate_physics();
///         world.apply_effects();
///         world.entities.len()
///     }, marker: {
///         name: "update world",
///     })
/// }
///
/// let mut world = World::new(13);
/// let count = update_world(&mut world);
///
/// assert_eq!(count, 13);
/// ```
///
/// #### Asynchronous
///
/// ```rust
/// # use samply_markers::samply_measure;
/// # async fn http_get(url: &str) -> Result<String, ()> { Ok(String::from("data")) }
/// # fn parse_response(data: &str) -> Result<Vec<String>, ()> { Ok(vec![]) }
/// // Measure the duration of fetch_user_data every time it is called.
/// async fn fetch_user_data(user_id: u64) -> Result<Vec<String>, ()> {
///     samply_measure!({
///         let response = http_get(&format!("/api/users/{user_id}")).await?;
///         parse_response(&response)
///     }, marker: {
///         name: "fetch user data",
///     })
/// }
/// ```
///
/// #### Create a New Async Block
///
/// Use the `async` keyword to create a new async block,
/// which allows the `?` operator to return from this block instead of the enclosing function.
///
/// ```rust
/// # use samply_markers::samply_measure;
/// # async fn read_file(path: &str) -> Option<String> { Some(String::from("100,200")) }
/// async fn load_config(path: &str) -> (u32, u32) {
///     let config = samply_measure!(async {
///         let contents = read_file(path).await?;
///         let mut parts = contents.split(',');
///
///         let x = parts.next()?.parse::<u32>().ok()?;
///         let y = parts.next()?.parse::<u32>().ok()?;
///
///         Some((x, y))
///     }, marker: {
///         name: "load config",
///     }).await;
///
///     config.unwrap_or((0, 0))
/// }
/// ```
///
/// #### Create a New Async Move Block
///
/// Use `async move` to transfer ownership of captured variables into the async block.
///
/// ```rust
/// # use samply_markers::samply_measure;
/// # async fn process_data(data: String) -> usize { data.len() }
/// async fn measure_owned_data() -> usize {
///     let data = String::from("owned data");
///     samply_measure!(async move {
///         process_data(data).await
///     }, marker: {
///         name: "process owned data",
///     }).await
/// }
/// ```
#[macro_export]
macro_rules! samply_measure {
    // New block scope within the same context
    (
        $body:block
        $(,)?
        marker: {
            name: $name:expr $(,)?
        }
        $(,)?
    ) => {{
        let _timer = $crate::marker::SamplyTimer::new($name);
        $body
    }};

    // Create a new async block
    (
        async $body:block
        $(,)?
        marker: {
            name: $name:expr $(,)?
        }
        $(,)?
    ) => {async {
        let _timer = $crate::marker::SamplyTimer::new($name);
        (async $body).await
    }};

    // Create a new async move block
    (
        async move $body:block
        $(,)?
        marker: {
            name: $name:expr $(,)?
        }
        $(,)?
    ) => {async move {
        let _timer = $crate::marker::SamplyTimer::new($name);
        (async move $body).await
    }};
}
