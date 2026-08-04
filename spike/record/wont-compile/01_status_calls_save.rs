// Attempt 1: the literal bug from bin/grind's cmd_status, transcribed.
//
//   state = load(run_id)   ->  let view = RunView::load(path)
//   observe(state)         ->  simulate_observe(...)
//   save(state)            ->  view.save(path)   <-- does not exist
//
// `RunView` is Deserialize-only. There is no `save` method to call.

use record::RunView;
use std::path::Path;

fn main() {
    let view = RunView::load(Path::new("../fixtures/run.json")).unwrap();
    view.save(Path::new("../fixtures/run.json")).unwrap();
}
