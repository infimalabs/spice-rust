use super::*;
use crate::composer_drafts::DraftImage;
use crate::image_annotation::{Editor as History, Kind, Point, Shape};
use std::cell::{Cell, RefCell};
use wasm_bindgen_futures::{JsFuture, spawn_local};

mod export;
mod keys;
mod pointer;
mod shape;
mod view;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Rectangle,
    Arrow,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Motion {
    Draw(usize),
    Move(usize, Shape, Point),
    Start(usize),
    End(usize),
}

#[derive(PartialEq, Eq)]
struct Gesture {
    pointer: i32,
    motion: Motion,
    before: Vec<Shape>,
}

pub(super) struct Applied {
    pub revision: u64,
    pub media: wire::Media,
    pub shapes: Rc<Vec<Shape>>,
}

#[derive(Properties, PartialEq)]
pub(super) struct EditorProps {
    pub image: DraftImage,
    pub onapply: Callback<Applied>,
    pub oncancel: Callback<()>,
    pub ondirty: Callback<bool>,
}

/// The editing session every handler shares. Its cells live as long as the
/// editor, so handlers compare by identity and read current state when called.
#[derive(Clone)]
struct Session {
    history: Rc<RefCell<History>>,
    gesture: Rc<RefCell<Option<Gesture>>>,
    /// Pointer capture can retarget a gesture's generated click to the SVG
    /// root. That click belongs to the edit, not to the surrounding backdrop.
    gesture_click: Rc<RefCell<bool>>,
    repaint: Callback<()>,
}

impl PartialEq for Session {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.history, &other.history)
            && Rc::ptr_eq(&self.gesture, &other.gesture)
            && Rc::ptr_eq(&self.gesture_click, &other.gesture_click)
            && self.repaint == other.repaint
    }
}

impl Session {
    fn owns(&self, event: &PointerEvent) -> bool {
        self.gesture
            .borrow()
            .as_ref()
            .is_some_and(|gesture| gesture.pointer == event.pointer_id())
    }
}

/// Decode the frozen original once for the intrinsic size that every mark and
/// the export are measured in.
#[hook]
fn use_natural_size(
    source: Rc<AttrValue>,
    issue: UseStateHandle<AttrValue>,
    alive: Rc<Cell<bool>>,
) -> (u32, u32) {
    let dimensions = use_state_eq(|| (0_u32, 0_u32));
    {
        let dimensions = dimensions.clone();
        use_effect_with(source, move |source| {
            let source = source.clone();
            let pending = alive.clone();
            spawn_local(async move {
                let result = async {
                    let image = web_sys::HtmlImageElement::new()?;
                    image.set_src(&source);
                    JsFuture::from(image.decode()).await?;
                    Ok::<_, JsValue>((image.natural_width(), image.natural_height()))
                }
                .await;
                if pending.get() {
                    match result {
                        Ok(size) => dimensions.set(size),
                        Err(_) => issue.set("Image unavailable for annotation".into()),
                    }
                }
            });
            move || alive.set(false)
        });
    }
    *dimensions
}

/// Focus the stage when the editor opens and track its laid-out size, which
/// keeps handles a constant on-screen size.
#[hook]
fn use_stage(surface: NodeRef) -> (f64, f64) {
    let viewport = use_state_eq(|| (0.0_f64, 0.0_f64));
    {
        let viewport = viewport.clone();
        use_effect_with(surface, move |surface| {
            let element = surface
                .cast::<web_sys::HtmlElement>()
                .expect("annotation surface");
            let _ = element.focus();
            let measured = element.clone();
            let resized = Closure::<dyn FnMut()>::new(move || {
                let bounds = measured.get_bounding_client_rect();
                viewport.set((bounds.width(), bounds.height()));
            });
            let observer = web_sys::ResizeObserver::new(resized.as_ref().unchecked_ref())
                .expect("annotation size observer");
            observer.observe(&element);
            move || {
                observer.disconnect();
                drop(resized);
            }
        });
    }
    *viewport
}

/// A toolbar history command: it abandons any gesture in progress first.
#[hook]
fn use_history_command(session: Session, command: fn(&mut History)) -> Callback<MouseEvent> {
    use_callback(session, move |_: MouseEvent, session| {
        {
            let mut history = session.history.borrow_mut();
            pointer::cancel(&mut history, &mut session.gesture.borrow_mut());
            command(&mut history);
        }
        session.repaint.emit(());
    })
}

struct Export {
    busy: UseStateHandle<bool>,
    issue: UseStateHandle<AttrValue>,
    alive: Rc<Cell<bool>>,
    onapply: Callback<Applied>,
}

/// Flatten the marks onto the frozen original at its native size and hand the
/// result back tagged with the revision the session opened, so the owner can
/// refuse it if that draft image has since changed or gone.
fn apply(base: &DraftImage, shapes: Rc<Vec<Shape>>, export: Export) {
    let original = base.original.clone();
    let revision = base.revision;
    export.busy.set(true);
    export.issue.set("Applying annotations".into());
    spawn_local(async move {
        let result = export::render(&original, &shapes).await;
        if !export.alive.get() {
            return;
        }
        export.busy.set(false);
        match result {
            Ok(media) => export.onapply.emit(Applied {
                revision,
                media,
                shapes,
            }),
            Err(error) => export.issue.set(
                format!(
                    "Annotations could not be applied: {}",
                    crate::browser::clipboard::error_text(&error)
                )
                .into(),
            ),
        }
    });
}

#[function_component(Editor)]
pub(super) fn editor(props: &EditorProps) -> Html {
    // Freeze the opening revision; a later draft change cannot silently become
    // the base of this editing session or claim its pending export.
    let base = use_state(|| props.image.clone());
    let source = use_memo(base.original.clone(), |image| {
        AttrValue::from(image.data_url.clone())
    });
    let refresh = use_force_update();
    let session = Session {
        history: use_mut_ref(|| History::new((*base.shapes).clone())),
        gesture: use_mut_ref(|| None::<Gesture>),
        gesture_click: use_mut_ref(|| false),
        repaint: use_callback((), move |(), ()| refresh.force_update()),
    };
    let tool = use_state_eq(|| Tool::Rectangle);
    let issue = use_state_eq(AttrValue::default);
    let busy = use_state_eq(|| false);
    let alive = use_mut_ref(|| Rc::new(Cell::new(true)));
    let dimensions = use_natural_size(source.clone(), issue.clone(), alive.borrow().clone());
    let surface = use_node_ref();
    let viewport = use_stage(surface.clone());
    let svg = use_node_ref();
    let canvas = pointer::DrawingSurface {
        svg: svg.clone(),
        dimensions,
    };
    let idle = !*busy;
    let down = use_callback(
        (
            canvas.clone(),
            surface.clone(),
            session.clone(),
            *tool,
            idle,
        ),
        |event: PointerEvent, (canvas, surface, session, tool, idle)| {
            if *idle {
                pointer::down(&event, canvas, surface, session, *tool);
            }
        },
    );
    let moved = use_callback(
        (canvas.clone(), session.clone()),
        |event: PointerEvent, (canvas, session)| pointer::moved(&event, canvas, session),
    );
    let up = use_callback(
        (canvas.clone(), session.clone()),
        |event: PointerEvent, (canvas, session)| pointer::up(&event, canvas, session),
    );
    let interrupted = use_callback(session.clone(), |event: PointerEvent, session| {
        pointer::interrupted(&event, session)
    });
    let keyboard = use_callback(
        (session.clone(), dimensions, idle),
        |event: KeyboardEvent, (session, dimensions, idle)| {
            if *idle {
                keys::handle(&event, session, *dimensions);
            }
        },
    );
    let backdrop = use_callback(
        (svg.clone(), session.clone()),
        |event: MouseEvent, (svg, session)| pointer::backdrop(&event, svg, session),
    );
    let actions = view::Actions {
        undo: use_history_command(session.clone(), History::undo),
        redo: use_history_command(session.clone(), History::redo),
        delete: use_history_command(session.clone(), History::delete),
        cancel: use_callback(props.oncancel.clone(), |_: MouseEvent, cancel| {
            cancel.emit(())
        }),
        apply: use_callback(
            (
                session.clone(),
                base.clone(),
                busy.clone(),
                issue.clone(),
                alive.clone(),
                props.onapply.clone(),
            ),
            |_: MouseEvent, (session, base, busy, issue, alive, onapply)| {
                if **busy || session.gesture.borrow().is_some() {
                    return;
                }
                let shapes = Rc::new(session.history.borrow().shapes.clone());
                let export = Export {
                    busy: busy.clone(),
                    issue: issue.clone(),
                    alive: alive.borrow().clone(),
                    onapply: onapply.clone(),
                };
                apply(base, shapes, export);
            },
        ),
    };
    let ready = dimensions.0 > 0 && dimensions.1 > 0;
    let history = session.history.borrow();
    let dirty = history.differs_from(base.shapes.as_ref());
    super::super::use_unsafe_intent_guard(dirty);
    {
        let ondirty = props.ondirty.clone();
        use_effect_with(dirty, move |dirty| ondirty.emit(*dirty));
    }
    html! {
        <>
            {view::toolbar(&tool, *busy, ready, &history, actions)}
            <div class="image-annotation-stage" ref={surface} tabindex="0" role="group" aria-label="Annotation canvas. Select marks to move; drag handles to adjust; arrow keys move the selected mark." onkeydown={keyboard}>
                if ready {
                    <svg ref={svg} class={classes!("image-annotation-svg", (*tool != Tool::Select).then_some("image-annotation-svg--draw"))} viewBox={format!("0 0 {} {}", dimensions.0, dimensions.1)} preserveAspectRatio="xMidYMid meet"
                        onpointerdown={down} onpointermove={moved} onpointerup={up} onpointercancel={interrupted.clone()} onlostpointercapture={interrupted} onclick={backdrop}>
                        <image href={(*source).clone()} x="0" y="0" width={dimensions.0.to_string()} height={dimensions.1.to_string()} />
                        {view::marks(&session, &history, dimensions, viewport)}
                    </svg>
                } else if issue.is_empty() {
                    <p class="image-annotation-loading" role="status">{"Loading image"}</p>
                }
            </div>
            if !issue.is_empty() { <p class="toast" role="status">{(*issue).clone()}</p> }
        </>
    }
}
