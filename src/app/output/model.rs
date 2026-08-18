#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shape {
    Pen(Vec<(i32, i32)>),
    Line((i32, i32), (i32, i32)),
    Rect((i32, i32), (i32, i32)),
    Text((i32, i32), String),
}

#[derive(Clone, Debug)]
pub(crate) struct Annotation {
    pub(crate) shape: Shape,
    pub(crate) color: [u8; 4],
}
