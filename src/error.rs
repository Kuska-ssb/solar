use std::{
    result::Result,
    error::Error,
    fmt::{self,Display,Formatter}
};

pub type SolarResult<T> = Result<T, Box<dyn Error + Sync + Send>>;

#[derive(Debug)]
pub struct SolarError(String);

impl Display for SolarError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for SolarError {
    
}

impl SolarError {
    pub fn new(msg : &str) -> SolarError{
        SolarError(msg.to_string())
    }
}
