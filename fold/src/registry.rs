fold_registry! {
    TaskEffort => "task::effort" {
        answer: "Closed task-phase effort and reason-coded unavailable evidence.";
        sources: [
            Facts(Task => "task::*"),
            Facts(Claim => "claim::*"),
            Facts(Lane => "lane::*"),
            Facts(Lineage => "lineage::*"),
            Facts(Journal => "journal::*"),
        ];
        beyond: Answer;
    }
}
