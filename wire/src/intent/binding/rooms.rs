//! The room family's rows of the command binding partitions.

use super::{AnswerShape, AnswerSource, Binding, BodyShape, CommandAnswer, CommandKind, Intent};

impl AnswerShape {
    pub(super) fn decode_room(
        self,
        body: impl AnswerSource,
    ) -> Result<CommandAnswer, serde_json::Error> {
        Ok(match self {
            Self::RoomPost => CommandAnswer::RoomPost(body.read()?),
            Self::RoomCoordination => CommandAnswer::RoomCoordination(body.read()?),
            Self::RoomLeave => CommandAnswer::RoomLeave(body.read()?),
            Self::RoomContribute => CommandAnswer::RoomContribute(body.read()?),
            Self::RoomContributionSources => CommandAnswer::RoomContributionSources(body.read()?),
            Self::RoomControl => CommandAnswer::RoomControl(body.read()?),
            Self::RoomConversationText => CommandAnswer::RoomConversationText(body.read()?),
            _ => unreachable!("room answer decode partition is closed"),
        })
    }
}

impl CommandKind {
    pub(super) const fn room_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::RoomPost => Binding::of(B::RoomPost, A::RoomPost),
            Self::RoomCoordinationRead => Binding::of(B::RoomCoordinationRead, A::RoomCoordination),
            Self::RoomConversationText => {
                Binding::of(B::RoomConversationText, A::RoomConversationText)
            }
            Self::RoomCoordinate => Binding::of(B::RoomCoordinate, A::RoomCoordination),
            Self::RoomPutAside => Binding::of(B::RoomPutAside, A::RoomCoordination),
            Self::RoomLeave => Binding::of(B::RoomLeave, A::RoomLeave),
            Self::RoomContribute => Binding::of(B::RoomContribute, A::RoomContribute),
            Self::RoomContributionSources => {
                Binding::of(B::RoomContributionSources, A::RoomContributionSources)
            }
            Self::RoomControl => Binding::of(B::RoomControl, A::RoomControl),
            Self::RoomClose => Binding::of(B::RoomClose, A::RoomControl),
            _ => panic!("room command binding partition is closed"),
        }
    }
}

impl Intent {
    pub(super) const fn room_shape(&self) -> BodyShape {
        match self {
            Self::RoomPost(_) => BodyShape::RoomPost,
            Self::RoomCoordinationRead(_) => BodyShape::RoomCoordinationRead,
            Self::RoomConversationText(_) => BodyShape::RoomConversationText,
            Self::RoomCoordinate(_) => BodyShape::RoomCoordinate,
            Self::RoomPutAside(_) => BodyShape::RoomPutAside,
            Self::RoomLeave(_) => BodyShape::RoomLeave,
            Self::RoomContribute(_) => BodyShape::RoomContribute,
            Self::RoomContributionSources(_) => BodyShape::RoomContributionSources,
            Self::RoomControl(_) => BodyShape::RoomControl,
            Self::RoomClose(_) => BodyShape::RoomClose,
            _ => panic!("room intent shape partition is closed"),
        }
    }
}

impl BodyShape {
    pub(super) fn decode_room_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::RoomPost => Intent::RoomPost(crate::decode(bytes)?),
            Self::RoomCoordinationRead => Intent::RoomCoordinationRead(crate::decode(bytes)?),
            Self::RoomConversationText => Intent::RoomConversationText(crate::decode(bytes)?),
            Self::RoomCoordinate => Intent::RoomCoordinate(crate::decode(bytes)?),
            Self::RoomPutAside => Intent::RoomPutAside(crate::decode(bytes)?),
            Self::RoomLeave => Intent::RoomLeave(crate::decode(bytes)?),
            Self::RoomContribute => Intent::RoomContribute(crate::decode(bytes)?),
            Self::RoomContributionSources => Intent::RoomContributionSources(crate::decode(bytes)?),
            Self::RoomControl => Intent::RoomControl(crate::decode(bytes)?),
            Self::RoomClose => Intent::RoomClose(crate::decode(bytes)?),
            _ => unreachable!("only room shapes decode as room bodies"),
        })
    }
}
