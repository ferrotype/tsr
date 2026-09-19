/// A rejected identity, ownership boundary or lazy graph publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidId,
    WrongOwner,
    InvalidSlot,
    Retired,
    InvalidGraph,
    TokenKindMismatch { cached: u32, requested: u32 },
    ReparsedParent,
    InvalidTokenRange,
    IdentityAdopted,
    Reentry,
}

impl std::fmt::Display for Error {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId => output.write_str("identity fields must be nonzero and within u32"),
            Self::WrongOwner => output.write_str("the scope does not retain this arena"),
            Self::InvalidSlot => output.write_str("the slot is not published in this arena"),
            Self::Retired => output.write_str("the checker generation has retired"),
            Self::InvalidGraph => {
                output.write_str("lazy roots must belong to the current transaction")
            }
            Self::TokenKindMismatch { cached, requested } => {
                write!(output, "token cache kind mismatch: {cached} != {requested}")
            }
            Self::ReparsedParent => output.write_str("cannot create token for a reparsed parent"),
            Self::InvalidTokenRange => output.write_str("token range is outside the source text"),
            Self::Reentry => {
                output.write_str("the checker identity is already leased by this thread")
            }
            Self::IdentityAdopted => {
                output.write_str("the checker identity already adopted its symbol arena")
            }
        }
    }
}

impl std::error::Error for Error {}
