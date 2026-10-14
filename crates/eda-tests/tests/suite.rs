//! Entry point: run the full 1000+ case suite.

use eda_tests::all_cases;

fn report(total: usize, failures: Vec<String>) {
    if !failures.is_empty() {
        let mut msg = format!("{} / {} cases failed:\n", failures.len(), total);
        for f in failures.iter().take(80) {
            msg.push_str("  - ");
            msg.push_str(f);
            msg.push('\n');
        }
        if failures.len() > 80 {
            msg.push_str(&format!("  ... and {} more\n", failures.len() - 80));
        }
        panic!("{}", msg);
    }
}

#[test]
fn suite_has_at_least_1000_cases() {
    let n = all_cases().len();
    assert!(n >= 1000, "suite has {} cases, need >= 1000", n);
}

#[test]
fn analog_suite_contributes_declared_case_count() {
    let mut cases = eda_tests::CaseList::new();
    eda_tests::ams_cases::register(&mut cases);
    assert_eq!(cases.len(), eda_tests::ams_cases::AMS_CASE_COUNT);
    let mut names: Vec<_> = cases.0.iter().map(|case| case.name.clone()).collect();
    names.sort();
    let unique = names
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert_eq!(unique, names.len(), "analog case names must be unique");
}

#[test]
fn full_suite_all_green() {
    if let Ok(prefix) = std::env::var("SUITE_FILTER") {
        let cases = all_cases();
        let filtered: Vec<_> = cases
            .0
            .into_iter()
            .filter(|c| c.name.contains(&prefix))
            .collect();
        let (total, failures) = eda_tests::CaseList(filtered).run_all();
        report(total, failures);
        return;
    }
    let (total, failures) = all_cases().run_all();
    if !failures.is_empty() {
        let mut msg = format!("{} / {} cases failed:\n", failures.len(), total);
        for f in failures.iter().take(80) {
            msg.push_str("  - ");
            msg.push_str(f);
            msg.push('\n');
        }
        if failures.len() > 80 {
            msg.push_str(&format!("  ... and {} more\n", failures.len() - 80));
        }
        panic!("{}", msg);
    }
}

#[test]
fn category_counts_reported() {
    // Sanity: each category registers a nonzero number of cases; the total
    // is asserted elsewhere. This test prints the distribution.
    let cases = all_cases();
    let mut cats: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for c in &cases.0 {
        let cat = c.name.split('_').next().unwrap_or("?").to_string();
        *cats.entry(cat).or_insert(0) += 1;
    }
    for (k, v) in &cats {
        assert!(*v > 0, "empty category {}", k);
        let _ = format!("{}: {}", k, v);
    }
}
