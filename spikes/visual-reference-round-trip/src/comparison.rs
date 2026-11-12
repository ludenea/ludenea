//! The three comparisons of a visual test. Throwaway, never product code.
//!
//! The verdict of a comparison comes from its comparator alone; what the
//! reference's record shows is the cause attached to that verdict, as a set of
//! facts. A comparison that fails writes pending parts and overwrites nothing;
//! a comparison that passes on a record gone stale writes a pending refresh;
//! and a run that takes references writes a pending byte part under a key
//! that has none.

use std::fs;
use std::path::Path;

use render_device_baseline::scene::Image;

use crate::pixels::{self, Bounds, Numbers};
use crate::reference::{BytePart, CONTENT_FILE, ContentPart, PENDING_SUFFIX, ReferenceSet, pixel_file_name};

/// What a device drew for a capture.
pub struct Rendered {
    pub key: String,
    pub image: Image,
    pub payloads: Vec<(String, u64)>,
    pub host_processor: String,
}

/// A capture: the content, and the picture where the device draws one.
pub struct Capture {
    pub content: ContentPart,
    /// Nothing on the content device, which draws no picture.
    pub rendered: Option<Rendered>,
}

/// One fact the record establishes about a capture and its reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fact {
    /// The part named does not exist.
    ReferenceMissing(&'static str),
    TargetDiffers,
    SettingsOrFixtureDiffer,
    /// With the first pass of the view that differs, where one does.
    ContentDiffers { view: String, pass: Option<String> },
    /// With each payload that differs from the byte part's.
    PayloadDiffers(Vec<String>),
    BytesDiffer,
    /// No byte part exists under the current key, and one does under another.
    KeyMoved,
    DeviceDrawsNothing,
}

impl Fact {
    pub fn name(&self) -> String {
        match self {
            Fact::ReferenceMissing(part) => format!("the reference does not exist ({part})"),
            Fact::TargetDiffers => "the target differs".to_string(),
            Fact::SettingsOrFixtureDiffer => "the presentation settings or the fixture's starting state differ".to_string(),
            Fact::ContentDiffers { view, pass: Some(pass) } => format!("the content differs (first in the view {view}, pass {pass})"),
            Fact::ContentDiffers { view, pass: None } => format!("the content differs (in the view {view}, in no pass)"),
            Fact::PayloadDiffers(names) => format!("a payload differs ({})", names.join(", ")),
            Fact::BytesDiffer => "the bytes differ".to_string(),
            Fact::KeyMoved => "the key moved".to_string(),
            Fact::DeviceDrawsNothing => "the device draws nothing".to_string(),
        }
    }

    /// The fact without its detail, for the group a review puts a pending
    /// part in.
    fn kind(&self) -> &'static str {
        match self {
            Fact::ReferenceMissing(_) => "the reference does not exist",
            Fact::TargetDiffers => "the target differs",
            Fact::SettingsOrFixtureDiffer => "the settings or the fixture differ",
            Fact::ContentDiffers { .. } => "the content differs",
            Fact::PayloadDiffers(_) => "a payload differs",
            Fact::BytesDiffer => "the bytes differ",
            Fact::KeyMoved => "the key moved",
            Fact::DeviceDrawsNothing => "the device draws nothing",
        }
    }
}

/// The group a review puts the pending parts of a cause in: the whole cause.
/// "The key moved" alone is the group of a key that moved and nothing else.
pub fn review_group(facts: &[Fact]) -> String {
    let mut kinds: Vec<&str> = facts.iter().map(Fact::kind).collect();
    kinds.dedup();
    if kinds.is_empty() { "nothing differs".to_string() } else { kinds.join(" and ") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compared {
    Equal,
    Different,
    /// No byte part under the current key, one under another.
    NotCompared,
    /// The device draws nothing to compare.
    NotMade,
}

impl Compared {
    pub fn name(self) -> &'static str {
        match self {
            Compared::Equal => "equal",
            Compared::Different => "different",
            Compared::NotCompared => "not compared",
            Compared::NotMade => "not made",
        }
    }
}

/// A pending part a comparison wrote, by what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pending {
    /// A content part where none is accepted, or beside one that differs.
    ContentPart,
    /// The content part again, written by a byte comparison that passed.
    ContentRefresh,
    /// A byte part where the comparison failed or none exists.
    BytePart,
    /// The byte part again, with the payloads now bound, written by a byte
    /// comparison that passed.
    ByteRefresh,
    /// A byte part under a key that had none, written by a run that takes
    /// references.
    BytePartUnderANewKey,
}

impl Pending {
    pub fn name(self) -> &'static str {
        match self {
            Pending::ContentPart => "content part",
            Pending::ContentRefresh => "refresh of the content part",
            Pending::BytePart => "byte part",
            Pending::ByteRefresh => "refresh of the byte part",
            Pending::BytePartUnderANewKey => "byte part under a new key",
        }
    }
}

pub struct Outcome {
    pub compared: Compared,
    pub passed: bool,
    /// The cause: every fact the record establishes for this comparison.
    pub facts: Vec<Fact>,
    pub pending: Vec<Pending>,
    /// The numbers of the difference, where two pictures were at hand.
    pub numbers: Option<Numbers>,
}

/// Where a comparison writes its pending parts, and whether the run takes
/// references.
pub struct Run<'a> {
    pub pending: &'a Path,
    pub takes_references: bool,
}

fn write_pending(run: &Run, name: &str, data: &[u8]) {
    // A pending file that cannot be written is reported by its absence from
    // the run's artifacts; the comparison's verdict does not depend on it.
    if fs::create_dir_all(run.pending).is_ok() {
        let _ = fs::write(run.pending.join(name), data);
    }
}

fn write_pending_content(run: &Run, capture: &Capture) {
    write_pending(run, &format!("{CONTENT_FILE}{PENDING_SUFFIX}"), capture.content.text().as_bytes());
}

/// Writes a pending byte part with its pixel file, and the difference picture
/// where a former picture is at hand.
fn write_pending_bytes(run: &Run, rendered: &Rendered, base: bool, former: Option<&Image>) {
    let file = pixels::write(&rendered.image);
    let name = pixel_file_name(&file);
    let part = BytePart {
        key: rendered.key.clone(),
        frame_bytes_hash: rendered.image.hash(),
        payloads: rendered.payloads.clone(),
        pixel_file: Some(name.clone()),
        base,
        host_processor: rendered.host_processor.clone(),
        note: None,
    };
    write_pending(run, &format!("{}{PENDING_SUFFIX}", BytePart::file_name(&part.key)), part.text().as_bytes());
    write_pending(run, &name, &file);
    if let Some(difference) = former.and_then(|former| pixels::difference_picture(former, &rendered.image)) {
        write_pending(run, &format!("difference-{name}"), &pixels::write(&difference));
    }
}

/// The facts about the content, the same for the three comparisons.
fn content_facts(capture: &Capture, set: &ReferenceSet) -> Vec<Fact> {
    let Some(reference) = &set.content else { return vec![Fact::ReferenceMissing("no content part")] };
    let mut facts = Vec::new();
    if reference.target != capture.content.target {
        facts.push(Fact::TargetDiffers);
    }
    if reference.presentation_settings_hash != capture.content.presentation_settings_hash || reference.fixture_hash != capture.content.fixture_hash {
        facts.push(Fact::SettingsOrFixtureDiffer);
    }
    let (accepted, captured) = (&reference.content, &capture.content.content);
    if accepted.hash != captured.hash || accepted.assets != captured.assets {
        let pass = captured.passes.iter().zip(&accepted.passes).find(|(new, old)| new != old).map(|(new, _)| new.0.clone());
        facts.push(Fact::ContentDiffers { view: captured.view.0.clone(), pass });
    }
    facts
}

fn content_is_as_accepted(facts: &[Fact]) -> bool {
    !facts.iter().any(|fact| matches!(fact, Fact::ReferenceMissing(_) | Fact::TargetDiffers | Fact::ContentDiffers { .. }))
}

/// Content equality: exact, under every key, with no pixel and no device.
pub fn content_equality(capture: &Capture, set: &ReferenceSet, run: &Run) -> Outcome {
    let facts = content_facts(capture, set);
    let passed = content_is_as_accepted(&facts);
    let mut pending = Vec::new();
    if !passed {
        write_pending_content(run, capture);
        pending.push(Pending::ContentPart);
    }
    Outcome { compared: if passed { Compared::Equal } else { Compared::Different }, passed, facts, pending, numbers: None }
}

/// Byte equality: exact under the key of the capture's device.
pub fn byte_equality(capture: &Capture, set: &ReferenceSet, run: &Run) -> Outcome {
    let mut facts = content_facts(capture, set);
    let content_as_accepted = content_is_as_accepted(&facts);
    let Some(rendered) = &capture.rendered else {
        // Not made; the content is still checked against the content part.
        facts.push(Fact::DeviceDrawsNothing);
        return Outcome { compared: Compared::NotMade, passed: content_as_accepted, facts, pending: Vec::new(), numbers: None };
    };
    if set.bytes.is_empty() {
        facts.push(Fact::ReferenceMissing("no byte part under any key"));
        write_pending_bytes(run, rendered, true, None);
        return Outcome { compared: Compared::Different, passed: false, facts, pending: vec![Pending::BytePart], numbers: None };
    }
    let target_differs = facts.contains(&Fact::TargetDiffers);
    let Some(part) = set.byte_part(&rendered.key) else {
        facts.push(Fact::KeyMoved);
        if target_differs {
            write_pending_bytes(run, rendered, false, None);
            return Outcome { compared: Compared::Different, passed: false, facts, pending: vec![Pending::BytePart], numbers: None };
        }
        // Not compared; the content is still checked, and a run that takes
        // references leaves a part under the new key for a review.
        let mut pending = Vec::new();
        if run.takes_references {
            write_pending_bytes(run, rendered, set.base().is_none(), None);
            pending.push(Pending::BytePartUnderANewKey);
        }
        return Outcome { compared: Compared::NotCompared, passed: content_as_accepted, facts, pending, numbers: None };
    };
    let moved: Vec<String> = rendered.payloads.iter().filter(|payload| !part.payloads.contains(payload)).map(|(name, _)| name.clone()).collect();
    let payloads_as_recorded = moved.is_empty() && rendered.payloads.len() == part.payloads.len();
    if !payloads_as_recorded {
        facts.push(Fact::PayloadDiffers(moved));
    }
    if !target_differs && part.frame_bytes_hash == rendered.image.hash() {
        // Equal bytes pass whatever the record shows; a record gone stale is
        // written again as a pending refresh.
        let mut pending = Vec::new();
        if facts.iter().any(|fact| matches!(fact, Fact::ContentDiffers { .. })) {
            write_pending_content(run, capture);
            pending.push(Pending::ContentRefresh);
        }
        if !payloads_as_recorded {
            let refreshed = BytePart { payloads: rendered.payloads.clone(), ..part.clone() };
            write_pending(run, &format!("{}{PENDING_SUFFIX}", BytePart::file_name(&part.key)), refreshed.text().as_bytes());
            pending.push(Pending::ByteRefresh);
        }
        return Outcome { compared: Compared::Equal, passed: true, facts, pending, numbers: None };
    }
    if !target_differs {
        facts.push(Fact::BytesDiffer);
    }
    let former = set.pixel_file(part).and_then(|file| pixels::read_strict(&file).ok());
    // The numbers of a failed byte comparison are measured, against no bound.
    let numbers = former.as_ref().and_then(|former| pixels::numbers(former, &rendered.image, &Bounds { largest: 0, second: 0, allowed_beyond_second: 0 }));
    write_pending_bytes(run, rendered, part.base, former.as_ref());
    Outcome { compared: Compared::Different, passed: false, facts, pending: vec![Pending::BytePart], numbers }
}

/// A bounded difference against the set's base, under any key.
pub fn bounded_difference(capture: &Capture, set: &ReferenceSet, bounds: &Bounds, run: &Run) -> Outcome {
    let mut facts = content_facts(capture, set);
    // The content is no part of this comparison's verdict; its absence is.
    facts.retain(|fact| !matches!(fact, Fact::ReferenceMissing(_)));
    let Some(rendered) = &capture.rendered else {
        facts.push(Fact::DeviceDrawsNothing);
        return Outcome { compared: Compared::NotMade, passed: true, facts, pending: Vec::new(), numbers: None };
    };
    let failed = |mut facts: Vec<Fact>, fact: Option<Fact>, former: Option<&Image>, numbers: Option<Numbers>| {
        facts.extend(fact);
        write_pending_bytes(run, rendered, set.base().is_none(), former);
        Outcome { compared: Compared::Different, passed: false, facts, pending: vec![Pending::BytePart], numbers }
    };
    let Some(base) = set.base() else { return failed(facts, Some(Fact::ReferenceMissing("no base")), None, None) };
    if base.key != rendered.key && set.byte_part(&rendered.key).is_none() {
        facts.push(Fact::KeyMoved);
    }
    let Some(former) = set.pixel_file(base).and_then(|file| pixels::read_strict(&file).ok()) else {
        return failed(facts, Some(Fact::ReferenceMissing("the base keeps no pixel file this reader reads")), None, None);
    };
    let Some(numbers) = pixels::numbers(&former, &rendered.image, bounds) else {
        if !facts.contains(&Fact::TargetDiffers) {
            facts.push(Fact::TargetDiffers);
        }
        return failed(facts, None, None, None);
    };
    if numbers.differing_pixels > 0 {
        facts.push(Fact::BytesDiffer);
    }
    if !numbers.within(bounds) {
        return failed(facts, None, Some(&former), Some(numbers));
    }
    Outcome { compared: if numbers.differing_pixels == 0 { Compared::Equal } else { Compared::Different }, passed: true, facts, pending: Vec::new(), numbers: Some(numbers) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Change, Frame};
    use crate::reference::write_accepted;
    use std::path::PathBuf;

    const BOUNDS: Bounds = Bounds { largest: 16, second: 4, allowed_beyond_second: 2 };

    fn picture(seed: u8) -> Image {
        Image { width: 8, height: 8, rgba: (0..8 * 8 * 4_u32).map(|byte| if byte % 4 == 3 { 255 } else { (byte as u8).wrapping_mul(3).wrapping_add(seed) }).collect() }
    }

    fn capture(change: Change, key: &str, image: Option<Image>) -> Capture {
        let frame = Frame::build(change);
        Capture {
            content: ContentPart::of(&frame.content(), &frame.target, Frame::fixture_hash(), frame.tick),
            rendered: image.map(|image| Rendered { key: key.to_string(), image, payloads: frame.payloads(), host_processor: "a processor".to_string() }),
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let directory = std::env::temp_dir().join(format!("visual-reference-comparison-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&directory);
            Scratch(directory)
        }

        /// A set accepted from the unchanged frame under `key`.
        fn accepted(&self, key: &str) -> ReferenceSet {
            let reference = capture(Change::None, key, Some(picture(0)));
            let rendered = reference.rendered.as_ref().expect("a picture");
            let file = pixels::write(&rendered.image);
            let bytes = BytePart { key: key.to_string(), frame_bytes_hash: rendered.image.hash(), payloads: rendered.payloads.clone(), pixel_file: Some(pixel_file_name(&file)), base: true, host_processor: String::new(), note: None };
            write_accepted(&self.0.join("set"), &reference.content, &bytes, &file).expect("the set is written");
            ReferenceSet::read(&self.0.join("set")).expect("the set is read")
        }

        fn run(&self) -> Run<'_> {
            Run { pending: &self.0, takes_references: true }
        }

        fn pending_files(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0).map(|entries| entries.flatten().filter(|entry| entry.path().is_file()).filter_map(|entry| entry.file_name().into_string().ok()).collect()).unwrap_or_default();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nothing_changed_passes_the_three_and_writes_nothing() {
        let scratch = Scratch::new("unchanged");
        let (set, capture) = (scratch.accepted("key"), capture(Change::None, "key", Some(picture(0))));
        let (content, bytes, bounded) = (content_equality(&capture, &set, &scratch.run()), byte_equality(&capture, &set, &scratch.run()), bounded_difference(&capture, &set, &BOUNDS, &scratch.run()));
        assert!(content.passed && bytes.passed && bounded.passed);
        assert_eq!((content.compared, bytes.compared, bounded.compared), (Compared::Equal, Compared::Equal, Compared::Equal));
        assert!(content.facts.is_empty() && bytes.facts.is_empty() && bounded.facts.is_empty());
        assert!(scratch.pending_files().is_empty());
    }

    #[test]
    fn a_reference_that_does_not_exist_fails_the_three_and_leaves_its_parts() {
        let scratch = Scratch::new("missing");
        let set = ReferenceSet::read(&scratch.0.join("absent")).expect("an absent set");
        let capture = capture(Change::None, "key", Some(picture(0)));
        assert!(!content_equality(&capture, &set, &scratch.run()).passed);
        assert!(!byte_equality(&capture, &set, &scratch.run()).passed);
        assert!(!bounded_difference(&capture, &set, &BOUNDS, &scratch.run()).passed);
        let files = scratch.pending_files();
        assert!(files.contains(&format!("{CONTENT_FILE}{PENDING_SUFFIX}")));
        assert!(files.iter().any(|name| name.starts_with("bytes-") && name.ends_with(PENDING_SUFFIX)));
        assert!(files.iter().any(|name| name.ends_with(".png")));
    }

    #[test]
    fn changed_content_with_equal_bytes_fails_content_and_passes_bytes_with_a_refresh() {
        let scratch = Scratch::new("outside");
        let (set, capture) = (scratch.accepted("key"), capture(Change::ItemMovedOutsideEveryView, "key", Some(picture(0))));
        let content = content_equality(&capture, &set, &scratch.run());
        assert!(!content.passed);
        assert_eq!(content.facts, vec![Fact::ContentDiffers { view: "main".to_string(), pass: Some("overlay".to_string()) }]);
        let bytes = byte_equality(&capture, &set, &scratch.run());
        assert!(bytes.passed);
        assert_eq!(bytes.pending, vec![Pending::ContentRefresh]);
        assert!(bounded_difference(&capture, &set, &BOUNDS, &scratch.run()).passed);
    }

    #[test]
    fn a_payload_that_changed_is_a_stale_reference_where_the_bytes_differ_and_a_refresh_where_they_do_not() {
        let scratch = Scratch::new("payload");
        let set = scratch.accepted("key");
        let differing = capture(Change::TextureEncodedAgain, "key", Some(picture(1)));
        assert!(content_equality(&differing, &set, &scratch.run()).passed);
        let bytes = byte_equality(&differing, &set, &scratch.run());
        assert!(!bytes.passed);
        assert_eq!(bytes.facts, vec![Fact::PayloadDiffers(vec!["gradient".to_string()]), Fact::BytesDiffer]);
        assert_eq!(bytes.numbers.map(|numbers| numbers.differing_pixels), Some(64));
        let equal = capture(Change::TextureEncodedAgainWhereNothingSamples, "key", Some(picture(0)));
        let bytes = byte_equality(&equal, &set, &scratch.run());
        assert!(bytes.passed);
        assert_eq!(bytes.pending, vec![Pending::ByteRefresh]);
    }

    #[test]
    fn another_key_is_not_compared_and_still_checks_the_content() {
        let scratch = Scratch::new("key");
        let set = scratch.accepted("the key of the reference");
        let same_content = capture(Change::None, "another key", Some(picture(0)));
        let bytes = byte_equality(&same_content, &set, &scratch.run());
        assert_eq!((bytes.compared, bytes.passed, bytes.pending.clone()), (Compared::NotCompared, true, vec![Pending::BytePartUnderANewKey]));
        assert_eq!(review_group(&bytes.facts), "the key moved");
        let silent = byte_equality(&same_content, &set, &Run { pending: &scratch.0, takes_references: false });
        assert!(silent.pending.is_empty());
        let changed = capture(Change::ItemMovedInsideTheView, "another key", Some(picture(9)));
        let bytes = byte_equality(&changed, &set, &scratch.run());
        assert_eq!((bytes.compared, bytes.passed), (Compared::NotCompared, false));
        assert_eq!(review_group(&bytes.facts), "the content differs and the key moved");
        let bounded = bounded_difference(&changed, &set, &BOUNDS, &scratch.run());
        assert!(!bounded.passed);
        assert!(bounded.facts.contains(&Fact::KeyMoved));
    }

    #[test]
    fn a_device_that_draws_nothing_compares_the_content_alone() {
        let scratch = Scratch::new("content-device");
        let set = scratch.accepted("key");
        let content_only = capture(Change::None, "", None);
        assert!(content_equality(&content_only, &set, &scratch.run()).passed);
        let bytes = byte_equality(&content_only, &set, &scratch.run());
        assert_eq!((bytes.compared, bytes.passed), (Compared::NotMade, true));
        assert_eq!(bounded_difference(&content_only, &set, &BOUNDS, &scratch.run()).compared, Compared::NotMade);
        let changed = capture(Change::ItemMovedInsideTheView, "", None);
        assert!(!byte_equality(&changed, &set, &scratch.run()).passed);
    }

    #[test]
    fn a_target_of_another_size_fails_the_three() {
        let scratch = Scratch::new("target");
        let set = scratch.accepted("key");
        let frame = Frame::build_at(Change::None, 512, 512);
        let capture = Capture {
            content: ContentPart::of(&frame.content(), &frame.target, Frame::fixture_hash(), frame.tick),
            rendered: Some(Rendered { key: "key".to_string(), image: Image { width: 16, height: 16, rgba: vec![0; 16 * 16 * 4] }, payloads: frame.payloads(), host_processor: String::new() }),
        };
        assert!(!content_equality(&capture, &set, &scratch.run()).passed);
        assert!(!byte_equality(&capture, &set, &scratch.run()).passed);
        assert!(!bounded_difference(&capture, &set, &BOUNDS, &scratch.run()).passed);
    }

    #[test]
    fn a_bounded_difference_is_decided_on_its_numbers() {
        let scratch = Scratch::new("bounded");
        let set = scratch.accepted("key");
        let mut near = picture(0);
        near.rgba[0] += 3;
        let passes = bounded_difference(&capture(Change::None, "key", Some(near)), &set, &BOUNDS, &scratch.run());
        assert_eq!((passes.compared, passes.passed), (Compared::Different, true));
        assert!(scratch.pending_files().is_empty());
        let fails = bounded_difference(&capture(Change::None, "key", Some(picture(40))), &set, &BOUNDS, &scratch.run());
        assert!(!fails.passed);
        assert!(scratch.pending_files().iter().any(|name| name.starts_with("difference-")));
    }
}
