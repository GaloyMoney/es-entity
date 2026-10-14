use es_entity::*;

pub struct Mine;

impl IdPolicy for Mine {
    type Slot<Id> = Id;
}

fn main() {}
