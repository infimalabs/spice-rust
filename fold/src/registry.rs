fold_registry! {
    SteerInbox => "steer::inbox" {
        answer: "Pending directives per lane.";
        content: [
            Facts(Steering => "steer::*"),
            Head(Witness => "witness"),
            Head(Residency => "residency"),
        ];
        sources: [
            Facts(Steering => "steer::*"),
            Head(Witness => "witness"),
            Head(Residency => "residency"),
        ];
        beyond: Wait;
    }
    ConsentQueue => "consent::queue" {
        answer: "Pending consent and remaining budgets.";
        content: [
            Facts(Consent => "consent::*"),
            Head(Witness => "witness"),
            Head(Residency => "residency"),
        ];
        sources: [
            Facts(Consent => "consent::*"),
            Head(Witness => "witness"),
            Head(Residency => "residency"),
            Config(Consent => "consent"),
        ];
        beyond: Wait;
    }
    LaneCanary => "lane::canary" {
        answer: "Armed windows, listening meters, and alarms.";
        content: [
            Facts(Canary => "canary::*"),
            Facts(Vendor => "vendor::*"),
            Head(Residency => "residency"),
        ];
        sources: [
            Facts(Canary => "canary::*"),
            Facts(Vendor => "vendor::*"),
            Head(Residency => "residency"),
            Tape(DeclaredExtents => "declared-extents"),
        ];
        beyond: Wait;
    }
    TasteActive => "taste::active" {
        answer: "Active tastes and evidence ages.";
        content: [Facts(Taste => "taste::*")];
        sources: [
            Facts(Taste => "taste::*"),
            Config(Taste => "taste"),
        ];
        beyond: Answer;
    }
    CanvasVerdicts => "canvas::verdicts" {
        answer: "Commit and finding verdict history.";
        content: [
            Facts(Commit => "commit::*"),
            Facts(Gate => "gate::*"),
            Head(Finding => "finding"),
        ];
        sources: [
            Facts(Commit => "commit::*"),
            Facts(Gate => "gate::*"),
            Head(Finding => "finding"),
        ];
        beyond: Answer;
    }
    CanvasMagnitude => "canvas::magnitude" {
        answer: "Effort, pressure, residue, and consent rates.";
        content: [
            Facts(Claim => "claim::*"),
            Facts(Consent => "consent::*"),
            Facts(Canary => "canary::*"),
            Facts(Vendor => "vendor::*"),
            Head(Task => "task"),
            Head(Claim => "claim"),
            Head(Lease => "lease"),
            Head(Residency => "residency"),
        ];
        sources: [
            Facts(Claim => "claim::*"),
            Facts(Consent => "consent::*"),
            Facts(Canary => "canary::*"),
            Facts(Vendor => "vendor::*"),
            Fold(LaneCanary => "lane::canary"),
            Head(Task => "task"),
            Head(Claim => "claim"),
            Head(Lease => "lease"),
            Head(Residency => "residency"),
        ];
        beyond: Answer;
    }
    CanvasFlow => "canvas::flow" {
        answer: "Tasks moving through phases.";
        content: [
            Facts(Task => "task::*"),
            Facts(Claim => "claim::*"),
            Facts(Document => "doc::*"),
            Head(Task => "task"),
            Head(Workflow => "workflow"),
            Head(Claim => "claim"),
            Head(Lease => "lease"),
        ];
        sources: [
            Facts(Task => "task::*"),
            Facts(Claim => "claim::*"),
            Facts(Document => "doc::*"),
            Head(Task => "task"),
            Head(Workflow => "workflow"),
            Head(Claim => "claim"),
            Head(Lease => "lease"),
        ];
        beyond: Answer;
    }
    CanvasTopology => "canvas::topology" {
        answer: "Lanes, worktrees, occupant lineage, and possession.";
        content: [
            Facts(Lane => "lane::*"),
            Fact(ClaimCarried => "claim::carried"),
            Fact(ClaimRenewed => "claim::renewed"),
            Facts(Resident => "up::*"),
            Head(Claim => "claim"),
            Head(Residency => "residency"),
        ];
        sources: [
            Facts(Lane => "lane::*"),
            Fact(ClaimCarried => "claim::carried"),
            Fact(ClaimRenewed => "claim::renewed"),
            Facts(Resident => "up::*"),
            Head(Claim => "claim"),
            Head(Residency => "residency"),
        ];
        beyond: Answer;
    }
    CanvasChronology => "canvas::chronology" {
        answer: "Compaction, queues, human delay, trust, recovery, and lifecycle transitions in time.";
        content: [
            Facts(Lane => "lane::*"),
            Facts(Steering => "steer::*"),
            Facts(Consent => "consent::*"),
            Facts(Canary => "canary::*"),
            Facts(Document => "doc::*"),
            Facts(Trust => "trust::*"),
            Facts(Resident => "up::*"),
            Facts(Journal => "journal::*"),
            Facts(Gate => "gate::*"),
            Head(Witness => "witness"),
            Head(Residency => "residency"),
        ];
        sources: [
            Facts(Lane => "lane::*"),
            Facts(Steering => "steer::*"),
            Facts(Consent => "consent::*"),
            Facts(Canary => "canary::*"),
            Facts(Document => "doc::*"),
            Facts(Trust => "trust::*"),
            Facts(Resident => "up::*"),
            Facts(Journal => "journal::*"),
            Facts(Gate => "gate::*"),
            CommandResults(Stored => "stored-command-results"),
            Fold(SteerInbox => "steer::inbox"),
            Fold(ConsentQueue => "consent::queue"),
            Fold(LaneCanary => "lane::canary"),
            Fold(CanvasVerdicts => "canvas::verdicts"),
            Ring(BusRefusals => "bus::refusals"),
            Head(Witness => "witness"),
            Head(Residency => "residency"),
        ];
        beyond: Answer;
    }
    ReleaseHistory => "release::history" {
        answer: "Sealed and published artifacts.";
        content: [
            Facts(Proof => "proof::*"),
            Facts(Release => "release::*"),
        ];
        sources: [
            Facts(Proof => "proof::*"),
            Facts(Release => "release::*"),
        ];
        beyond: Answer;
    }
}

ring_registry! {
    BusRefusals => "bus::refusals" {
        recovery: None;
    }
}
