use crate::serializers::borsh::BorshSerialize;

/// Trait for converting a message data to a string.
pub trait Message: BorshSerialize {
    /// Returns the topic of the message.
    fn topic(&self) -> &str;
    /// Converts the message data to a string.
    fn payload(&self) -> Vec<u8>;
}

#[cfg(test)]
mod tests {
    use crate::serializers::borsh::BorshSerialize;

    use super::Message;

    #[derive(BorshSerialize)]
    struct MyEvent {
        value1: u64,
        value2: String,
    }

    impl Message for MyEvent {
        fn topic(&self) -> &str {
            "my_event"
        }

        fn payload(&self) -> Vec<u8> {
            let mut data = Vec::new();
            self.serialize(&mut data).unwrap();
            data
        }
    }

    fn emit(message: impl Message) {
        let _topic = message.topic();
        let _payload = message.payload();
        // Emit the message to the topic
    }

    #[test]
    fn test() {
        let event = MyEvent {
            value1: 42,
            value2: "Hello, World!".to_string(),
        };

        emit(event);
    }
}
