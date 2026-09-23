use recurse_protocol::{CellResult, CellStatus, Event};

/// Renders a cell as sections: stdout, stderr, result, error, then a status line.
pub fn cell(cell: &CellResult) -> String {
    let mut sections = Vec::new();

    if !cell.stdout.is_empty() {
        sections.push(format!("stdout:\n{}", cell.stdout.trim_end_matches('\n')));
    }

    if !cell.stderr.is_empty() {
        sections.push(format!("stderr:\n{}", cell.stderr.trim_end_matches('\n')));
    }

    if let Some(result) = &cell.result {
        sections.push(format!("result:\n{result}"));
    }

    if let Some(error) = &cell.error {
        sections.push(format!(
            "error: {}: {}\n{}",
            error.ename,
            error.evalue,
            error.traceback.trim_end_matches('\n')
        ));
    }

    let truncated = if cell.truncated {
        ", output truncated"
    } else {
        ""
    };

    sections.push(format!(
        "status: {} (execution_count {}, {} ms{truncated})",
        cell.status.as_str(),
        cell.execution_count,
        cell.duration_ms
    ));

    sections.join("\n\n")
}

pub const fn is_error(cell: &CellResult) -> bool {
    !matches!(cell.status, CellStatus::Ok)
}

pub fn pending_events(events: &[Event]) -> Option<String> {
    let actionable = events.iter().filter(|event| event.actionable).count();

    (!events.is_empty()).then(|| {
        format!(
            "[recurse] {} queued event(s) ({actionable} actionable); call events_read, handle them, then events_ack.",
            events.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use recurse_protocol::{CellError, CellResult, CellStatus};

    #[test]
    fn formats_every_section() {
        let mut cell = CellResult::empty(CellStatus::Error, 2, 5);

        cell.stdout = "hi\n".into();
        cell.error = Some(CellError {
            ename: "ZeroDivisionError".into(),
            evalue: "division by zero".into(),
            traceback: "Traceback…".into(),
        });

        assert_eq!(
            super::cell(&cell),
            "stdout:\nhi\n\nerror: ZeroDivisionError: division by zero\nTraceback…\n\nstatus: error (execution_count 2, 5 ms)"
        );
        assert!(super::is_error(&cell));
    }
}
