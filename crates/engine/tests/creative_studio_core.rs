//! Creative Studio MVP smoke coverage for the core editing surface.
//!
//! Dedicated engine/UI tests cover these commands in much greater depth. This test keeps the
//! product roadmap baseline explicit: ordinary layer editing, pixel masks and editable type must
//! all work together through the public Session command boundary.

use photocraft_engine::Session;
use serde_json::json;

#[test]
fn core_layer_mask_and_text_workflow_is_editable() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 96, "height": 64})).unwrap();

    // Layer basics: create, rename/properties, duplicate and reorder.
    let hero = s.execute("layer.new.layer", json!({"name": "Product"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setProps", json!({"layer": hero, "name": "Hero", "opacity": 0.75})).unwrap();
    let copy = s.execute("layer.duplicate", json!({"layer": hero})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.arrange.sendBackward", json!({"layer": copy})).unwrap();

    {
        let doc = &s.active().unwrap().doc;
        let hero_layer = doc.layer(photocraft_doc::LayerId(hero)).unwrap();
        assert_eq!(hero_layer.name, "Hero");
        assert!((hero_layer.opacity - 0.75).abs() < 1e-6);
        assert!(doc.layer(photocraft_doc::LayerId(copy)).is_some());
    }

    // Pixel-mask basics: add, disable/unlink and delete without flattening the source layer.
    // Mask toggle enablement follows the active layer, just like the UI, so select the target first.
    s.execute("layer.select", json!({"layer": hero})).unwrap();
    s.execute("layer.layerMask.revealAll", json!({"layer": hero})).unwrap();
    s.execute("layer.layerMask.enabled", json!({"layer": hero, "enabled": false})).unwrap();
    s.execute("layer.layerMask.linked", json!({"layer": hero, "linked": false})).unwrap();
    {
        let mask = s.active().unwrap().doc.layer(photocraft_doc::LayerId(hero)).unwrap().mask.as_ref().unwrap();
        assert!(!mask.enabled);
        assert!(!mask.linked);
    }
    s.execute("layer.layerMask.delete", json!({"layer": hero})).unwrap();
    assert!(s.active().unwrap().doc.layer(photocraft_doc::LayerId(hero)).unwrap().mask.is_none());

    // Editable type: create a type layer, replace its text and change style while keeping it live.
    let label = s.execute("type.create", json!({"x": 10, "y": 42, "text": "Original", "size": 18, "color": "#202020", "name": "Label"})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    s.execute("type.edit", json!({"layer": label, "text": "New label", "name": "Product label"})).unwrap();
    s.execute("type.setStyle", json!({"layer": label, "size": 28, "color": "#336699"})).unwrap();
    let info = s.execute("type.info", json!({"layer": label})).unwrap();
    assert_eq!(info["text"], "New label");
    assert_eq!(info["runs"][0]["style"]["size_pt"], 28.0);
    assert_eq!(s.active().unwrap().doc.layer(photocraft_doc::LayerId(label)).unwrap().name, "Product label");

    // Deleting a duplicate must not disturb the live hero/text layers.
    s.execute("layer.delete", json!({"layer": copy})).unwrap();
    let doc = &s.active().unwrap().doc;
    assert!(doc.layer(photocraft_doc::LayerId(copy)).is_none());
    assert!(doc.layer(photocraft_doc::LayerId(hero)).is_some());
    assert!(doc.layer(photocraft_doc::LayerId(label)).is_some());
}
