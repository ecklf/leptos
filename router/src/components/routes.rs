use std::{cmp::Reverse, rc::Rc, cell::{RefCell, Cell}, ops::IndexMut};

use leptos::*;
use typed_builder::TypedBuilder;

use crate::{matching::{expand_optionals, join_paths, Branch, Matcher, RouteDefinition, get_route_matches, RouteMatch}, RouterContext, RouteContext};

#[derive(TypedBuilder)]
pub struct RoutesProps {
    #[builder(default, setter(strip_option))]
    base: Option<String>,
    children: Box<dyn Fn() -> Vec<RouteDefinition>>,
}

#[allow(non_snake_case)]
pub fn Routes(cx: Scope, props: RoutesProps) -> impl IntoChild {
    let router = use_context::<RouterContext>(cx).unwrap_or_else(|| {
        log::warn!("<Routes/> component should be nested within a <Router/>.");
        panic!()
    });

    let mut branches = Vec::new();
    create_branches(
        &(props.children)(),
        &props.base.unwrap_or_default(),
        &mut Vec::new(),
        &mut branches,
    );

    // whenever path changes, update matches
    let matches = create_memo(cx, {
        let router = router.clone();
        move |_| {
            get_route_matches(branches.clone(), router.pathname().get())
        }
    });

    // Rebuild the list of nested routes conservatively, and show the root route here
    let mut disposers = Vec::<ScopeDisposer>::new();

    // iterate over the new matches, reusing old routes when they are the same
    // and replacing them with new routes when they differ
    let next: Rc<RefCell<Vec<RouteContext>>> = Default::default();

    let root_equal = Rc::new(Cell::new(true));

    let route_states: Memo<RouterState> = create_memo(cx, {
        let root_equal = root_equal.clone();
        move |prev: Option<RouterState>| {
            root_equal.set(true);
            next.borrow_mut().clear();

            let next_matches = matches();
            let prev_matches = prev.as_ref().map(|p| &p.matches);
            let prev_routes = prev.as_ref().map(|p| &p.routes);

            // are the new route matches the same as the previous route matches so far?
            let mut equal = prev_matches
                .map(|prev_matches| next_matches.len() == prev_matches.len())
                .unwrap_or(false);

            for i in 0..next_matches.len() {
                let next = next.clone();
                let prev_match = prev_matches.and_then(|p| p.get(i));
                let next_match = next_matches.get(i).unwrap();

                if let Some(prev) = prev_routes && let Some(prev_match) = prev_match && next_match.route.key == prev_match.route.key {
                    let prev_one = { prev.borrow()[i].clone() };
                    if i >= next.borrow().len() {
                        next.borrow_mut().push(prev_one);
                    } else {
                        *(next.borrow_mut().index_mut(i)) = prev_one;
                    }
                } else {
                    equal = false; 
                    if i == 0 {
                        root_equal.set(false);
                    }

                    let disposer = cx.child_scope({
                        let next = next.clone();
                        let router = Rc::clone(&router.inner);
                        move |cx| {
                            let next = next.clone();
                            let next_ctx = RouteContext::new(
                                cx,
                                &RouterContext { inner: router },
                                {
                                    let next = next.clone();
                                    move || {
                                        next.borrow().get(i + 1).cloned()
                                    }
                                },
                                move || {
                                    matches().get(i).cloned()
                                }
                            );

                            if let Some(next_ctx) = next_ctx {
                                if next.borrow().len() > i + 1 {
                                    next.borrow_mut()[i] = next_ctx;
                                } else {
                                    next.borrow_mut().push(next_ctx);
                                }
                            }
                        }
                    });

                    if disposers.len() > i + 1 {
                        let old_route_disposer = std::mem::replace(&mut disposers[i], disposer);
                        old_route_disposer.dispose();
                    } else {
                        disposers.push(disposer);
                    }
                }
            }

            // TODO dispose of extra routes from previous matches if they're longer than new ones

            if let Some(prev) = &prev && equal {
                RouterState {
                    matches: next_matches.to_vec(),
                    routes: prev_routes.cloned().unwrap_or_default(),
                    root: prev.root.clone()
                }
            } else {
                let root = next.borrow().get(0).cloned();
                RouterState {
                    matches: next_matches.to_vec(),
                    routes: Rc::new(RefCell::new(next.borrow().to_vec())),
                    root
                }
            }
        }
    });

    // show the root route
    create_memo(cx, move |prev| {
        route_states.with(|state| {
            let root = state.routes.borrow();
            let root = root.get(0);
            if let Some(route) = root {
                provide_context(cx, route.clone());
            }

            if prev.is_none() || !root_equal.get() {
                root.as_ref().map(|route| route.outlet().into_child(cx))
            } else {
                prev.clone().unwrap()
            }
        })
    })
}

#[derive(Clone, Debug, PartialEq)]
struct RouterState {
    matches: Vec<RouteMatch>,
    routes: Rc<RefCell<Vec<RouteContext>>>,
    root: Option<RouteContext>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteData {
    pub key: RouteDefinition,
    pub pattern: String,
    pub original_path: String,
    pub matcher: Matcher,
}

impl RouteData {
    fn score(&self) -> i32 {
        let (pattern, splat) = match self.pattern.split_once("/*") {
            Some((p, s)) => (p, Some(s)),
            None => (self.pattern.as_str(), None),
        };
        let segments = pattern
            .split('/')
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>();
        segments.iter().fold(
            (segments.len() as i32) - if splat.is_none() { 0 } else { 1 },
            |score, segment| score + if segment.starts_with(':') { 2 } else { 3 },
        )
    }
}

fn create_branches(
    route_defs: &[RouteDefinition],
    base: &str,
    stack: &mut Vec<RouteData>,
    branches: &mut Vec<Branch>,
) {
    for def in route_defs {
        let routes = create_routes(def, base);
        for route in routes {
            stack.push(route.clone());

            if def.children.is_empty() {
                let branch = create_branch(stack, branches.len());
                branches.push(branch);
            } else {
                create_branches(&def.children, &route.pattern, stack, branches);
            }

            stack.pop();
        }
    }

    if stack.is_empty() {
        branches.sort_by_key(|branch| Reverse(branch.score));
    }
}

pub(crate) fn create_branch(routes: &[RouteData], index: usize) -> Branch {
    Branch {
        routes: routes.to_vec(),
        score: routes.last().unwrap().score() * 10000 - (index as i32),
    }
}

fn create_routes(route_def: &RouteDefinition, base: &str) -> Vec<RouteData> {
    let RouteDefinition { children, .. } = route_def;
    let is_leaf = children.is_empty();
    let mut acc = Vec::new();
    for original_path in expand_optionals(route_def.path) {
        let path = join_paths(base, &original_path);
        let pattern = if is_leaf {
            path
        } else {
            path.split("/*")
                .next()
                .map(|n| n.to_string())
                .unwrap_or(path)
        };
        acc.push(RouteData {
            key: route_def.clone(),
            matcher: Matcher::new_with_partial(&pattern, !is_leaf),
            pattern,
            original_path: original_path.to_string(),
        });
    }
    acc
}
