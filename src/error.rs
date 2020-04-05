pub type SolarResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;
