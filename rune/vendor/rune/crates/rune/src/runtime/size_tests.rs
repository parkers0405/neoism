use super::Node;
use crate::El;

/// Every builder call moves an `El`, and flatten moves each into a `Node`, so their sizes are
/// paid per element per rebuild. Fields most elements leave unset live behind a pointer.
#[test]
fn elements_and_nodes_stay_small() {
    let (el, node) = (std::mem::size_of::<El>(), std::mem::size_of::<Node>());
    eprintln!(
        "SIZES El={el} Node={node} Visual={} CompactStyle={}",
        std::mem::size_of::<crate::el::Visual>(),
        std::mem::size_of::<rune_taffy::CompactStyle>(),
    );
    assert!(el <= 800 && node <= 512, "El is {el} bytes and Node {node}");
}
