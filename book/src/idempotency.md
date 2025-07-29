# Idempotency

Idempotency means that performing the same operation multiple times has the same effect as doing it once.
It’s used to ensure that retrying a request doesn’t cause unintended side effects, such as duplicated `Event`s being persisted.

It is particularly useful in the context of a distributed system where operations could be triggered from an asynchronous event queue (ie pub-sub).
Whenever you would like to have an `exactly-once` processing guarantee - you can easily achieve an `effectively-once` processing by ensuring your mutations are all idempotent.

Making your `Entity` mutations idempotent is very simple when doing Event Sourcing as you can easily check if the event you are about to append already exists in the history.

## Example

To see the issue in action - lets consider the `update_name` mutation without an idempotency check.

```rust
pub enum UserEvent {
    Initialized { id: u64, name: String },
    NameUpdated { name: String },
}

pub struct User {
    events: Vec<UserEvent>
}

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) {
        let name = new_name.into();
        self.events.push(UserEvent::NameUpdated { name });
    }
}
```

In the above code we could easily record redundant events by calling the `update_name` mutation multiple times with the same input.
```rust
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
# impl User {
#     pub fn update_name(&mut self, new_name: impl Into<String>) {
#         let name = new_name.into();
#         self.events.push(UserEvent::NameUpdated { name });
#     }
# }

fn main() {
    let mut user = User { events: vec![] };
    user.update_name("Harrison");

    // Causes a redundant event to be appended
    user.update_name("Harrison");

    assert_eq!(user.events.len(), 2);
}
```

To prevent this we can iterate through the events to check if it has already been applied:

```rust
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) {
        let name = new_name.into();
        for event in self.events.iter().rev() {
            match event {
                UserEvent::NameUpdated { name: existing_name } if existing_name == &name => {
                    return;
                }
                _ => ()
            }
        }
        self.events.push(UserEvent::NameUpdated { name });
    }
}

fn main() {
    let mut user = User { events: vec![] };

    user.update_name("Harrison");

    // This update will be ignored
    user.update_name("Harrison");

    assert_eq!(user.events.len(), 1);
}
```

But now we just silently ignore the operation.
Better would be to signal back to the caller whether or not the operation was applied.
For that we use the `Idempotent` type:
```rust
# extern crate es_entity;
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
use es_entity::Idempotent;
// #[must_use]
// pub enum Idempotent<T> {
//     Executed(T),
//     Ignored,
// }

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()>{
        let name = new_name.into();
        for event in self.events.iter().rev() {
            match event {
                UserEvent::NameUpdated { name: existing_name } if existing_name == &name => {
                    return Idempotent::Ignored;
                }
                _ => ()
            }
        }
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}

fn main() {
    let mut user = User { events: vec![] };
    assert!(user.update_name("Harrison").did_execute());
    assert!(user.update_name("Harrison").was_ignored());
}
```

To cut down on boilerplate this pattern of iterating the events to check if an event was already applied has been encoded into the `idempotency_guard!` macro:

```rust
# extern crate es_entity;
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
use es_entity::{idempotency_guard, Idempotent};

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()>{
        let name = new_name.into();
        idempotency_guard!(
            // The iterator of events
            self.events.iter().rev(),
            // The pattern match to check whether an operation was already applied
            UserEvent::NameUpdated { name: existing_name } if existing_name == &name 
        );
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}

fn main() {
    let mut user = User { events: vec![] };
    assert!(user.update_name("Harrison").did_execute());
    assert!(user.update_name("Harrison").was_ignored());
}
```

Finally there is the case where an operation was applied in the past - but it is still legal to re-apply it.
Like changing a name back to what it originally was:
```rust
# extern crate es_entity;
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
use es_entity::{idempotency_guard, Idempotent};

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()>{
        let name = new_name.into();
        idempotency_guard!(
            self.events.iter().rev(),
            UserEvent::NameUpdated { name: existing_name } if existing_name == &name,
            // The `=>` signifies the pattern where to stop the iteration.
            => UserEvent::NameUpdated { .. }
        );
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}

fn main() {
    let mut user = User { events: vec![] };
    assert!(user.update_name("Harrison").did_execute());
    assert!(user.update_name("Colin").did_execute());
    assert!(user.update_name("Harrison").did_execute());
}
```

Without the `=>` argument the second call of `assert!(user.update_name("Harrison").did_execute());` would fail.
