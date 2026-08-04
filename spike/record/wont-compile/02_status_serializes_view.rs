// Attempt 2: sidestep the missing `save` method and try to write the bytes
// by hand instead, the way a status command reaching for "just dump it back"
// might. `RunView` never derives `Serialize`, so there is no impl to call.

use record::RunView;
use std::path::Path;

fn main() {
    let view = RunView::load(Path::new("../fixtures/run.json")).unwrap();
    let body = serde_json::to_string(&view).unwrap();
    std::fs::write("../fixtures/run.json", body).unwrap();
}
