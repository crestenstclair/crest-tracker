//! Delivery of the immutable selected phrase. The browser retains one exact
//! snapshot; acknowledgements suppress source-sized playback/cursor payloads.
//! A new token on context change or resync rejects delayed acknowledgements.
use super::*;

#[derive(Clone, Serialize)]
struct Source {
    token: String,
    revision: u64,
    phrase: Option<PhraseId>,
}

#[derive(Default)]
pub(super) struct PhraseDelivery {
    source: Option<Source>,
    acknowledged: bool,
}

impl PhraseDelivery {
    fn prepare(&mut self, revision: u64, phrase: Option<PhraseId>) -> (Source, bool) {
        if self
            .source
            .as_ref()
            .is_none_or(|source| source.revision != revision || source.phrase != phrase)
        {
            self.source = Some(Source {
                token: uuid::Uuid::new_v4().to_string(),
                revision,
                phrase,
            });
            self.acknowledged = false;
        }
        (
            self.source.as_ref().expect("prepared source").clone(),
            !self.acknowledged,
        )
    }

    fn acknowledge(&mut self, token: &str) {
        if self
            .source
            .as_ref()
            .is_some_and(|source| source.token == token)
        {
            self.acknowledged = true;
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    #[serde(rename = "kind")]
    _kind: String,
    token: String,
}

impl App {
    /// Complete transient projection plus an explicitly identified phrase source.
    /// Full/headless projections stay self-contained. Only browser delivery may
    /// omit a phrase that this page has acknowledged under the current token.
    pub fn browser_projection(&mut self) -> Value {
        let phrase = self.selected_phrase().map(|(id, _)| id);
        let (source, include) = self.phrase_delivery.prepare(self.editor.revision(), phrase);
        let mut projection = self.build_projection(include);
        projection["phraseSource"] = json!(source);
        projection
    }

    /// Presentation acknowledgements bypass musical commands, file dialogs and
    /// edit gestures. Invalid messages do not change delivery or musical state.
    pub(super) fn receive_phrase_delivery(&mut self, value: &Value) -> bool {
        match value.get("kind").and_then(Value::as_str) {
            Some("phrase_projection_ack") => {
                match serde_json::from_value::<Acknowledgement>(value.clone()) {
                    Ok(ack) if uuid::Uuid::parse_str(&ack.token).is_ok() => {
                        self.phrase_delivery.acknowledge(&ack.token);
                    }
                    _ => self.status = "Invalid phrase projection acknowledgement".into(),
                }
            }
            Some("phrase_projection_resync") => {
                if value.as_object().is_some_and(|fields| fields.len() == 1) {
                    self.phrase_delivery = PhraseDelivery::default();
                } else {
                    self.status = "Invalid phrase projection resync".into();
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acknowledge(app: &mut App, projection: &Value) {
        app.queue(
            json!({"kind":"phrase_projection_ack", "token":projection["phraseSource"]["token"]}),
        );
    }

    #[test]
    fn phrase_delivery_requires_exact_ack_and_resynchronizes_after_reload() {
        let mut app = App::new();
        let first = app.browser_projection();
        assert_eq!(first["phrase"], Value::Null);
        assert!(app.browser_projection().get("phrase").is_some());
        app.queue(
            json!({"kind":"phrase_projection_ack", "token":uuid::Uuid::new_v4().to_string()}),
        );
        assert!(app.browser_projection().get("phrase").is_some());
        acknowledge(&mut app, &first);
        assert!(app.browser_projection().get("phrase").is_none());
        app.queue(json!({"kind":"phrase_projection_resync"}));
        let reset = app.browser_projection();
        assert_ne!(
            reset["phraseSource"]["token"],
            first["phraseSource"]["token"]
        );
        acknowledge(&mut app, &first);
        assert!(app.browser_projection().get("phrase").is_some());
        acknowledge(&mut app, &reset);
        assert!(app.browser_projection().get("phrase").is_none());
        assert!(app.requests.is_empty());
        assert!(!app.editor.can_undo());
        assert!(!app.dirty());
    }

    #[test]
    fn source_changes_undo_redo_and_replacement_each_require_a_full_phrase() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let initial = app.browser_projection();
        assert!(initial["phrase"].is_object());
        acknowledge(&mut app, &initial);
        app.playing = true;
        app.play_beat = Some(Beat::ONE);
        let compact = app.browser_projection();
        assert!(compact.get("phrase").is_none());
        assert_eq!(compact["playing"], true);
        let mut restored = compact.clone();
        restored["phrase"] = initial["phrase"].clone();
        restored.as_object_mut().unwrap().remove("phraseSource");
        assert_eq!(restored, app.projection());
        let phrase = app.selected_phrase().unwrap().0;
        let event = Event::note(
            EventId::new(9007199254740993).unwrap(),
            Beat::new(1, 3).unwrap(),
            Target::Channel {
                channel: Channel::FIRST,
                endpoint: None,
            },
            Note::new(67, 88, Beat::new(3, 8).unwrap()).unwrap(),
        );
        app.command(Command::PutEvent {
            phrase,
            event: event.clone(),
        })
        .unwrap();
        let changed = app.browser_projection();
        assert!(changed.get("phrase").is_some());
        assert_eq!(changed["phrase"]["value"]["events"], json!([event]));
        acknowledge(&mut app, &changed);
        for (request, count) in [(Request::Undo, 0), (Request::Redo, 1)] {
            app.apply(request).unwrap();
            let next = app.browser_projection();
            assert!(next.get("phrase").is_some());
            assert_eq!(
                next["phrase"]["value"]["events"].as_array().unwrap().len(),
                count
            );
            assert_ne!(
                next["phraseSource"]["token"],
                changed["phraseSource"]["token"]
            );
            acknowledge(&mut app, &next);
        }
        app.replace_song(app.editor.song().clone()).unwrap();
        assert!(app.browser_projection().get("phrase").is_some());
    }

    #[test]
    fn switching_context_and_stale_or_malformed_messages_cannot_hide_source() {
        let mut delivery = PhraseDelivery::default();
        let a = Some(PhraseId::new(1).unwrap());
        let b = Some(PhraseId::new(2).unwrap());
        let (first, _) = delivery.prepare(4, a);
        delivery.acknowledge(&first.token);
        assert!(!delivery.prepare(4, a).1);
        assert!(delivery.prepare(4, b).1);
        let (returned, include) = delivery.prepare(4, a);
        assert!(include);
        assert_ne!(returned.token, first.token);
        delivery.acknowledge(&first.token);
        assert!(delivery.prepare(4, a).1);
        let mut app = App::new();
        let source = app.browser_projection();
        app.queue(json!({"kind":"phrase_projection_ack", "token":source["phraseSource"]["token"], "extra":true}));
        assert!(app.browser_projection().get("phrase").is_some());
        app.queue(json!({"kind":"phrase_projection_resync", "extra":true}));
        assert_eq!(
            app.browser_projection()["phraseSource"],
            source["phraseSource"]
        );
        assert!(app.requests.is_empty());
    }
}
