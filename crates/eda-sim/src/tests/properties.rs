//! Concurrent properties, assumptions, delays, and repetition tests.

use super::*;

#[test]
fn concurrent_property_samples_on_clock_edge() {
    let mut sim = sim_for(
        "module m(input clk, req, ready, output grant);
            assign grant = req & ready;
            assert property (@(posedge clk) req |-> grant)
                else $error(\"request must be granted\");
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("ready", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("ready", &LogicWord::from_u64(1, 0)).unwrap();
    let error = sim
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("assertion failed"));
    assert!(error.contains("request must be granted"));
}

#[test]
fn nonoverlapping_property_checks_the_next_sample() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |=> grant)
            else $error(\"grant must follow request\");
     endmodule";

    let mut passing = sim_for(source);
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();

    let mut failing = sim_for(source);
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("grant must follow request"));
}

#[test]
fn sequence_composition_properties_execute() {
    let mut and_property = sim_for(
        "module m(input clk, req, first, second);
            assert property (@(posedge clk) req |-> first and second)
                else $error(\"both sequences must match\");
         endmodule",
    );
    and_property
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    and_property
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    and_property
        .set_input("first", &LogicWord::from_u64(1, 1))
        .unwrap();
    and_property
        .set_input("second", &LogicWord::from_u64(1, 1))
        .unwrap();
    and_property
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    and_property
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    and_property
        .set_input("first", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = and_property
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("both sequences must match"));

    let mut or_property = sim_for(
        "module m(input clk, req, first, second);
            assert property (@(posedge clk) req |-> first or second)
                else $error(\"one sequence must match\");
         endmodule",
    );
    or_property
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    or_property
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    or_property
        .set_input("first", &LogicWord::from_u64(1, 0))
        .unwrap();
    or_property
        .set_input("second", &LogicWord::from_u64(1, 1))
        .unwrap();
    or_property
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    or_property
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    or_property
        .set_input("first", &LogicWord::from_u64(1, 0))
        .unwrap();
    or_property
        .set_input("second", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = or_property
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("one sequence must match"));

    let mut delayed_sequence = sim_for(
        "module m(input clk, req, first, second);
            assert property (@(posedge clk) req |-> first ##[2] second)
                else $error(\"delayed sequence must match\");
         endmodule",
    );
    delayed_sequence
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_sequence
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    delayed_sequence
        .set_input("first", &LogicWord::from_u64(1, 1))
        .unwrap();
    delayed_sequence
        .set_input("second", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_sequence
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    for edge in 0..2 {
        delayed_sequence
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_sequence
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_sequence
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_sequence
            .set_input("second", &LogicWord::from_u64(1, u64::from(edge == 1)))
            .unwrap();
        delayed_sequence
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }

    let mut delayed_or = sim_for(
        "module m(input clk, req, first, second);
            assert property (@(posedge clk) req |-> first or ##[2] second)
                else $error(\"delayed alternative must match\");
         endmodule",
    );
    delayed_or
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_or
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    delayed_or
        .set_input("first", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_or
        .set_input("second", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_or
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    for edge in 0..2 {
        delayed_or
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_or
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_or
            .set_input("first", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_or
            .set_input("second", &LogicWord::from_u64(1, u64::from(edge == 1)))
            .unwrap();
        delayed_or
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }

    let mut delayed_cover = sim_for(
        "module m(input clk, req, first, second);
            composed: cover property (@(posedge clk) req |-> ##[2] first and second);
         endmodule",
    );
    delayed_cover
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_cover
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    delayed_cover
        .set_input("first", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_cover
        .set_input("second", &LogicWord::from_u64(1, 0))
        .unwrap();
    delayed_cover
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    for edge in 0..2 {
        delayed_cover
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        delayed_cover
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        let final_edge = edge == 1;
        delayed_cover
            .set_input("first", &LogicWord::from_u64(1, u64::from(final_edge)))
            .unwrap();
        delayed_cover
            .set_input("second", &LogicWord::from_u64(1, u64::from(final_edge)))
            .unwrap();
        delayed_cover
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    assert_eq!(delayed_cover.property_cover_hits("composed"), 1);
}

#[test]
fn assume_and_cover_properties_execute() {
    let mut assumption = sim_for(
        "module m(input clk, req, grant);
            assume property (@(posedge clk) req |-> grant)
                else $error(\"grant is required\");
         endmodule",
    );
    assumption
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    assumption
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    assumption
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = assumption
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.starts_with("assumption failed"));
    assert!(error.contains("grant is required"));

    let mut negedge_assumption = sim_for(
        "module m(input clk, req, grant);
            assume property (@(negedge clk) req |-> grant);
         endmodule",
    );
    negedge_assumption
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    negedge_assumption
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    negedge_assumption
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = negedge_assumption
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap_err();
    assert!(error.starts_with("assumption failed"));

    let mut cover = sim_for(
        "module m(input clk, req, grant);
            sample_grant: cover property (@(posedge clk) req |=> grant);
         endmodule",
    );
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("sample_grant"), 0);
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("sample_grant"), 1);
}

#[test]
fn fixed_delay_property_checks_after_specified_edges() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |-> ##[2] grant)
            else $error(\"grant must arrive two edges later\");
     endmodule";

    let mut passing = sim_for(source);
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();

    let mut failing = sim_for(source);
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("grant must arrive two edges later"));
}

#[test]
fn delay_range_property_accepts_any_cycle_in_window() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |-> ##[1:3] grant)
            else $error(\"grant must arrive within three edges\");
     endmodule";

    let mut passing = sim_for(source);
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();

    let mut failing = sim_for(source);
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    for _ in 0..3 {
        failing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
    }
    let error = failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("grant must arrive within three edges"));
}

#[test]
fn consecutive_repetition_requires_successive_samples() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |-> grant[*3])
            else $error(\"grant must remain high for three edges\");
     endmodule";

    let mut passing = sim_for(source);
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    for _ in 0..2 {
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, 1))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();

    let mut failing = sim_for(source);
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    for grant in [1, 0] {
        failing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        failing
            .set_input("grant", &LogicWord::from_u64(1, grant))
            .unwrap();
        if grant == 0 {
            let error = failing
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap_err();
            assert!(error.contains("grant must remain high for three edges"));
        } else {
            failing
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }
    }
}

#[test]
fn repetition_range_matches_at_minimum_consecutive_count() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |-> grant[*2:3])
            else $error(\"grant needs two successive edges\");
     endmodule";

    let mut passing = sim_for(source);
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();

    let mut failing = sim_for(source);
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    failing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("req", &LogicWord::from_u64(1, 0))
        .unwrap();
    failing
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    let error = failing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap_err();
    assert!(error.contains("grant needs two successive edges"));
}

#[test]
fn goto_repetition_counts_nonconsecutive_samples() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |-> grant[->3])
            else $error(\"grant must be seen three times\");
     endmodule";

    let mut passing = sim_for(source);
    passing
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    passing
        .set_input("req", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    passing
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    for grant in [0, 1, 0, 1] {
        passing
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("req", &LogicWord::from_u64(1, 0))
            .unwrap();
        passing
            .set_input("grant", &LogicWord::from_u64(1, grant))
            .unwrap();
        passing
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }

    let mut cover = sim_for(
        "module m(input clk, req, grant);
            seen_three: cover property (@(posedge clk) req |-> grant[->3]);
         endmodule",
    );
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("seen_three"), 0);
    for grant in [0, 1, 0, 1] {
        cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        cover
            .set_input("grant", &LogicWord::from_u64(1, grant))
            .unwrap();
        cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }
    assert_eq!(cover.property_cover_hits("seen_three"), 1);
}

#[test]
fn goto_repetition_range_matches_at_minimum_count() {
    let source = "module m(input clk, req, grant);
        assert property (@(posedge clk) req |-> grant[->2:4])
            else $error(\"grant must be seen twice to four times\");
     endmodule";

    let mut sim = sim_for(source);
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    for grant in [0, 1] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, grant))
            .unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    let mut cover = sim_for(
        "module m(input clk, req, grant);
            range_seen: cover property (@(posedge clk) req |-> grant[->2:4]);
         endmodule",
    );
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("range_seen"), 0);
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("range_seen"), 1);
}

#[test]
fn nonconsecutive_repetition_matches_and_retires() {
    let mut sim = sim_for(
        "module m(input clk, req, grant);
            assert property (@(posedge clk) req |-> grant[=2:4])
                else $error(\"grant must be nonconsecutively seen twice\");
         endmodule",
    );
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();

    for _ in 0..3 {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("grant", &LogicWord::from_u64(1, 1)).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    }

    let mut cover = sim_for(
        "module m(input clk, req, grant);
            nonconsecutive_seen: cover property (@(posedge clk) req |-> grant[=2]);
         endmodule",
    );
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 1)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("nonconsecutive_seen"), 0);
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover.set_input("req", &LogicWord::from_u64(1, 0)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 0))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    cover
        .set_input("grant", &LogicWord::from_u64(1, 1))
        .unwrap();
    cover.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    assert_eq!(cover.property_cover_hits("nonconsecutive_seen"), 1);
}
