//! Checks the shared file name order fixture against the comparison every host
//! tree uses.

use lithe_core_rules::file_names::compare_file_names;
use serde_json::Value;
use std::cmp::Ordering;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/workspace/file-name-order-v1.json"
    ))
    .expect("file-name order fixture should be JSON")
}

#[test]
fn shared_fixture_groups_sort_strictly_in_the_listed_order() {
    let fixture = fixture();
    for group in fixture["orderedGroups"].as_array().unwrap() {
        let names = group["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect::<Vec<_>>();
        for (index, earlier) in names.iter().enumerate() {
            for later in &names[index + 1..] {
                assert_eq!(
                    compare_file_names(earlier, later),
                    Ordering::Less,
                    "{}: {earlier:?} should precede {later:?}",
                    group["id"]
                );
                assert_eq!(
                    compare_file_names(later, earlier),
                    Ordering::Greater,
                    "{}: {later:?} should follow {earlier:?}",
                    group["id"]
                );
            }
        }
    }
}

// Rust's sort may panic on an inconsistent order, so the port must stay a total
// preorder even for the characters upstream treats specially: spaces, zeroes,
// digits, '#', '-', '_' and mixed-case letters. The generator is a fixed LCG,
// so a failure reproduces exactly.
#[test]
fn comparison_is_antisymmetric_and_transitive_for_special_characters() {
    const ALPHABET: [char; 11] = [' ', '0', '1', '2', 'a', 'A', 'b', '-', '_', '#', '.'];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as usize) % bound
    };
    let names = (0..72)
        .map(|_| {
            let length = next(6);
            (0..length)
                .map(|_| ALPHABET[next(ALPHABET.len())])
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    for left in &names {
        for right in &names {
            assert_eq!(
                compare_file_names(left, right),
                compare_file_names(right, left).reverse(),
                "{left:?} against {right:?}"
            );
        }
    }
    for first in &names {
        for second in &names {
            if compare_file_names(first, second) == Ordering::Greater {
                continue;
            }
            for third in &names {
                if compare_file_names(second, third) != Ordering::Greater {
                    assert_ne!(
                        compare_file_names(first, third),
                        Ordering::Greater,
                        "{first:?} <= {second:?} <= {third:?}"
                    );
                }
            }
        }
    }
}
