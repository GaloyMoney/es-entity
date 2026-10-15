use es_entity::*;

pub struct Mine;

impl IdSource for Mine {
    type Slot<Id> = Id;
}

fn main() {}
