//! Error reporting. For now very stupid and simplistic.

use itertools::Itertools;
use grammar::repr::*;
use message::{Message};
use message::builder::{Builder, BodyCharacter, MessageBuilder};
use util::{Map, map};
use lr1::trace::Tracer;
use lr1::core::*;
use lr1::example::{Example, ExampleStyles, ExampleSymbol};
use lr1::lookahead::{Lookahead, LookaheadSet};
use tls::Tls;

#[cfg(test)] mod test;

pub fn report_error(grammar: &Grammar,
                    error: &TableConstructionError)
                    -> Vec<Message>
{
    let mut cx = ErrorReportingCx::new(grammar, &error.states);
    cx.report_errors()
}

struct ErrorReportingCx<'cx, 'grammar: 'cx> {
    grammar: &'grammar Grammar,
    states: &'cx [State<'grammar>],
}

#[derive(Debug)]
enum ConflictClassification {
    /// The grammar is ambiguous. This means we have two examples of
    /// precisely the same set of symbols which can be reduced in two
    /// distinct ways.
    Ambiguity { action: Example, reduce: Example },

    /// The grammar is ambiguous, and moreover it looks like a
    /// precedence error. This means that the reduction is to a
    /// nonterminal `T` and the shift is some symbol sandwiched
    /// between two instances of `T`.
    Precedence { shift: Example, reduce: Example, nonterminal: NonterminalString },

    /// Suggest inlining `nonterminal`. Makes sense if there are two
    /// levels in the reduction tree in both examples, and the suffix
    /// after the inner reduction is the same in all cases.
    SuggestInline { shift: Example, reduce: Example,
                    nonterminal: NonterminalString },

    /// Like the previous, but suggest replacing `nonterminal` with
    /// `symbol?`. Makes sense if the thing to be inlined consists of
    /// two alternatives, `X = symbol | ()`.
    SuggestQuestion { shift: Example, reduce: Example,
                      nonterminal: NonterminalString, symbol: Symbol },

    /// Can't say much beyond that a conflict occurred.
    InsufficientLookahead { action: Example, reduce: Example },

    /// Really can't say *ANYTHING*.
    Naive,
}

impl<'cx, 'grammar> ErrorReportingCx<'cx, 'grammar> {
    fn new(grammar: &'grammar Grammar,
           states: &'cx [State<'grammar>])
           -> Self {
        ErrorReportingCx {
            grammar: grammar,
            states: states,
        }
    }

    fn report_errors(&mut self) -> Vec<Message> {
        self.states
            .iter()
            .flat_map(|state|
                      &state.conflicts)
            .flat_map(|(&lookahead, conflicts)|
                      conflicts.iter().map(move |c| (lookahead, c)))
            .map(|(lookahead, conflict)|
                 self.report_error(lookahead, conflict))
            .collect()
    }

    fn report_error(&mut self,
                    lookahead: Lookahead,
                    conflict: &Conflict<'grammar>)
                    -> Message
    {
        match self.classify(lookahead, conflict) {
            ConflictClassification::Ambiguity { action, reduce } => {
                self.report_error_ambiguity(conflict, action, reduce)
            }
            ConflictClassification::Precedence { shift, reduce, nonterminal } => {
                self.report_error_precedence(conflict, shift, reduce, nonterminal)
            }
            ConflictClassification::SuggestInline { shift, reduce, nonterminal } => {
                self.report_error_suggest_inline(lookahead, conflict,
                                                 shift, reduce,
                                                 nonterminal)
            }
            ConflictClassification::SuggestQuestion { shift, reduce,
                                                      nonterminal, symbol } => {
                self.report_error_suggest_question(lookahead, conflict,
                                                   shift, reduce,
                                                   nonterminal, symbol)
            }
            ConflictClassification::InsufficientLookahead { action, reduce } => {
                self.report_error_insufficient_lookahead(lookahead, conflict,
                                                         action, reduce)
            }
            ConflictClassification::Naive => {
                self.report_error_naive(lookahead, conflict)
            }
        }
    }

    fn report_error_ambiguity_core(&self,
                                   conflict: &Conflict<'grammar>,
                                   shift: Example,
                                   reduce: Example)
                                   -> Builder<BodyCharacter> {
        let styles = ExampleStyles::ambig();
        MessageBuilder::new(conflict.production.span)
            .heading()
            .text("Ambiguous grammar detected")
            .end()
            .body()

            .lines()
            .wrap_text("The following symbols can be reduced in two ways:")
            .push(reduce.to_symbol_list(reduce.symbols.len(), styles))
            .end()

            .lines()
            .wrap_text("They could be reduced like so:")
            .push(reduce.into_picture(styles))
            .end()

            .lines()
            .wrap_text("Alternatively, they could be reduced like so:")
            .push(shift.into_picture(styles))
            .end()
    }

    fn report_error_ambiguity(&self,
                              conflict: &Conflict<'grammar>,
                              shift: Example,
                              reduce: Example)
                              -> Message {
        self.report_error_ambiguity_core(conflict, shift, reduce)
            .wrap_text("LALRPOP does not yet support ambiguous grammars. \
                        See the LALRPOP manual for advice on \
                        making your grammar unambiguous.")
            .end()
            .end()
    }

    fn report_error_precedence(&self,
                               conflict: &Conflict<'grammar>,
                               shift: Example,
                               reduce: Example,
                               nonterminal: NonterminalString)
                               -> Message {
        self.report_error_ambiguity_core(conflict, shift, reduce)
            .wrap()
            .text("This looks like a precedence error related to")
            .push(nonterminal)
            .verbatimed()
            .punctuated(".")
            .text("See the LALRPOP manual for advice on encoding precedence.")
            .end()
            .end()
            .end()
    }

    fn report_error_not_lr1_core(&self,
                                 lookahead: Lookahead,
                                 conflict: &Conflict<'grammar>,
                                 action: Example,
                                 reduce: Example)
                                 -> Builder<BodyCharacter> {
        let styles = ExampleStyles::new();
        let builder =
            MessageBuilder::new(conflict.production.span)
            .heading()
            .text("Local ambiguity detected")
            .end()

            .body()
            .lines()
            .wrap_text("The grammar as written cannot be parsed using an LR(1) \
                        parser, as more than one token of lookahead would be required. \
                        After encountering the following symbols in the input:")
            .push(if action.cursor >= reduce.cursor {
                action.to_symbol_list(action.cursor, styles)
            } else {
                reduce.to_symbol_list(reduce.cursor, styles)
            });

        let builder = builder.wrap();

        let builder = match lookahead {
            Lookahead::Terminal(term) => {
                builder
                    .text("and when looking at the terminal")
                    .push(term)
                    .verbatimed()
                    .styled(Tls::session().cursor_symbol)
                    .punctuated(",")
            }
            Lookahead::EOF => {
                builder
                    .text("and when there are no more input tokens,")
            }
        };

        let builder =
            builder.text("parsing could continue in two distinct ways.")
                   .end()
                   .end();

        let builder = builder.lines();
        let builder = match conflict.action {
            Action::Shift(_) =>
                builder.wrap_text("First, the parser could take no action yet, \
                                   leading to:"),
            Action::Reduce(production) =>
                builder.wrap()
                       .text("First, the parser could reduce")
                       .push(production.nonterminal)
                       .verbatimed()
                       .punctuated(",")
                       .text("leading to:")
                       .end(),
        };
        let builder =
            builder.push(action.into_picture(styles))
                   .end();

        builder
            .lines()
            .wrap()
            .text("Second, the parser could reduce")
            .push(reduce.reductions[0].nonterminal)
            .verbatimed()
            .punctuated(",")
            .text("leading to:")
            .end()
            .push(reduce.into_picture(styles))
            .wrap_text("(Note that an LR(1) parser must execute reductions \
                        as soon as it can.)")
            .end()
    }

    fn report_error_suggest_inline(&self,
                                   lookahead: Lookahead,
                                   conflict: &Conflict<'grammar>,
                                   shift: Example,
                                   reduce: Example,
                                   nonterminal: NonterminalString)
                                   -> Message
    {
        let builder = self.report_error_not_lr1_core(lookahead, conflict,
                                                     shift, reduce);

        builder
            .wrap()
            .text("It appears you could resolve this problem by adding")
            .text("the annotation `#[inline]` to the definition of")
            .push(nonterminal)
            .verbatimed()
            .punctuated(".")
            .text("For more information, see the section on inlining")
            .text("in the LALROP manual.")
            .end()
            .end()
            .end()
    }

    fn report_error_suggest_question(&self,
                                     lookahead: Lookahead,
                                     conflict: &Conflict<'grammar>,
                                     shift: Example,
                                     reduce: Example,
                                     nonterminal: NonterminalString,
                                     symbol: Symbol)
                                     -> Message
    {
        let builder = self.report_error_not_lr1_core(lookahead, conflict,
                                                     shift, reduce);

        builder
            .wrap()
            .text("It appears you could resolve this problem by replacing")
            .text("uses of")
            .push(nonterminal)
            .text("with")
            .push(symbol)
            .adjacent_text("`", "?`")
            .text("(or, alternatively, by adding the annotation `#[inline]` \
                   to the definition of")
            .push(nonterminal)
            .end()
            .wrap_text("For more information, see the section on inlining \
                        in the LALROP manual.")
            .end()
            .end()
    }

    fn report_error_insufficient_lookahead(&self,
                                           lookahead: Lookahead,
                                           conflict: &Conflict<'grammar>,
                                           action: Example,
                                           reduce: Example)
                                           -> Message {
        let builder = self.report_error_not_lr1_core(lookahead, conflict,
                                                     action, reduce);

        builder
            .wrap_text("See the LALRPOP manual for advice on \
                        making your grammar LR(1).")
            .end()
            .end()
    }

    /// Naive error reporting. This is a fallback path which (I think)
    /// never actually executes.
    fn report_error_naive(&self,
                          lookahead: Lookahead,
                          conflict: &Conflict<'grammar>)
                          -> Message {
        let mut builder =
            MessageBuilder::new(conflict.production.span)
            .heading()
            .text("Conflict detected")
            .end()
            .body()
            .lines()
            .wrap_text("when in this state:")
            .indented();
        for item in self.states[conflict.state.0].items.vec.iter() {
            builder = builder.text(format!("{:?}", item));
        }
        let mut builder =
            builder.end()
                   .wrap()
                   .text(format!("and looking at a token `{:?}`", lookahead))
                   .text("we can reduce to a")
                   .push(conflict.production.nonterminal)
                   .verbatimed();
        builder = match conflict.action {
            Action::Shift(_) =>
                builder.text("but we can also shift"),
            Action::Reduce(prod) =>
                builder.text("but we can also reduce to a")
                       .text(prod.nonterminal)
                       .verbatimed()
        };
        builder.end()
               .end()
               .end()
    }

    fn classify(&mut self,
                lookahead: Lookahead,
                conflict: &Conflict<'grammar>)
                -> ConflictClassification
    {
        // Find examples from the conflicting action (either a shift
        // or a reduce).
        let mut action_examples = match conflict.action {
            Action::Shift(_) => self.shift_examples(lookahead, conflict),
            Action::Reduce(production) => self.reduce_examples(conflict.state,
                                                               production,
                                                               lookahead)
        };

        // Find examples from the conflicting reduce.
        let mut reduce_examples = self.reduce_examples(conflict.state,
                                                       conflict.production,
                                                       lookahead);

        // Prefer shorter examples to longer ones.
        action_examples.sort_by(|e, f| e.symbols.len().cmp(&f.symbols.len()));
        reduce_examples.sort_by(|e, f| e.symbols.len().cmp(&f.symbols.len()));

        if let Some(classification) = self.try_classify_ambiguity(lookahead,
                                                                  conflict,
                                                                  &action_examples,
                                                                  &reduce_examples) {
            return classification;
        }

        if let Some(classification) = self.try_classify_inline(lookahead,
                                                               conflict,
                                                               &action_examples,
                                                               &reduce_examples) {
            return classification;
        }

        // Give up. Just grab an example from each and pair them up.
        // If there aren't even two examples, something's pretty
        // bogus, but we'll just call it naive.
        action_examples
            .into_iter()
            .zip(reduce_examples)
            .next()
            .map(|(action, reduce)| {
                ConflictClassification::InsufficientLookahead {
                    action: action,
                    reduce: reduce,
                }
            })
            .unwrap_or(ConflictClassification::Naive)
    }

    fn try_classify_ambiguity(&self,
                              lookahead: Lookahead,
                              conflict: &Conflict<'grammar>,
                              action_examples: &[Example],
                              reduce_examples: &[Example])
                              -> Option<ConflictClassification> {
        action_examples
            .iter()
            .cartesian_product(reduce_examples)
            .filter(|&(action, reduce)| action.symbols == reduce.symbols)
            .filter(|&(action, reduce)| action.cursor == reduce.cursor)
            .map(|(action, reduce)| {
                // Consider whether to call this a precedence
                // error. We do this if we are stuck between reducing
                // `T = T S T` and shifting `S`.
                if let Action::Shift(_) = conflict.action {
                    if let Lookahead::Terminal(term) = lookahead {
                        let nt = conflict.production.nonterminal;
                        if conflict.production.symbols.len() == 3 &&
                            conflict.production.symbols[0] == Symbol::Nonterminal(nt) &&
                            conflict.production.symbols[1] == Symbol::Terminal(term) &&
                            conflict.production.symbols[2] == Symbol::Nonterminal(nt)
                        {
                            return ConflictClassification::Precedence {
                                shift: action.clone(),
                                reduce: reduce.clone(),
                                nonterminal: conflict.production.nonterminal,
                            };
                        }
                    }
                }
                ConflictClassification::Ambiguity {
                    action: action.clone(),
                    reduce: reduce.clone()
                }
            })
            .next()
    }

    fn try_classify_inline(&self,
                           _lookahead: Lookahead,
                           _conflict: &Conflict<'grammar>,
                           action_examples: &[Example],
                           reduce_examples: &[Example])
                           -> Option<ConflictClassification> {
        action_examples
            .iter()
            .cartesian_product(reduce_examples)
            .filter(|&(action, _)| action.reductions.len() == 2)
            .filter(|&(_, reduce)| reduce.reductions.len() == 2)
            .filter(|&(_, reduce)|
                    reduce.reductions[0].nonterminal !=
                    reduce.reductions[1].nonterminal)
            .filter(|&(action, reduce)| {
                let action_suffix = self.inner_suffix(action);
                let reduce_suffix = self.inner_suffix(reduce);
                action_suffix == reduce_suffix
            })
            .map(|(action, reduce)| {
                let nt = reduce.reductions[0].nonterminal;
                let nt_productions = self.grammar.productions_for(nt);
                if nt_productions.len() == 2 {
                    for &(i, j) in &[(0, 1), (1, 0)] {
                        if
                            nt_productions[i].symbols.is_empty() &&
                            nt_productions[j].symbols.len() == 1
                        {
                            return ConflictClassification::SuggestQuestion {
                                shift: action.clone(),
                                reduce: reduce.clone(),
                                nonterminal: nt,
                                symbol: nt_productions[j].symbols[0],
                            }
                        }
                    }
                }
                ConflictClassification::SuggestInline {
                    shift: action.clone(),
                    reduce: reduce.clone(),
                    nonterminal: nt,
                }
            })
            .next()
    }

    fn inner_suffix<'ex>(&self, example: &'ex Example) -> &'ex [ExampleSymbol] {
        let end = example.reductions[0].end;
        &example.symbols[end..]
    }

    fn shift_examples(&self,
                      lookahead: Lookahead,
                      conflict: &Conflict<'grammar>)
                      -> Vec<Example> {
        log!(Tls::session(), Verbose, "Gathering shift examples");
        let state = &self.states[conflict.state.0];
        let conflicting_items = self.conflicting_shift_items(state, lookahead, conflict);
        conflicting_items
            .into_iter()
            .flat_map(|(item, _lookaheads)| {
                let tracer = Tracer::new(self.grammar, self.states);
                let shift_trace =
                    tracer.backtrace_shift(conflict.state, item);
                let local_examples: Vec<Example> =
                    shift_trace.examples(item).collect();
                local_examples
            })
            .collect()
    }

    fn reduce_examples(&self,
                       state: StateIndex,
                       production: &'grammar Production,
                       lookahead: Lookahead)
                       -> Vec<Example> {
        log!(Tls::session(), Verbose, "Gathering reduce examples");
        let item = Item {
            production: production,
            index: production.symbols.len(),
            lookahead: lookahead
        };
        let tracer = Tracer::new(self.grammar, self.states);
        let reduce_trace = tracer.backtrace_reduce(state, item);
        reduce_trace.examples(item.to_lr0()).collect()
    }

    fn conflicting_shift_items(&self,
                               state: &State<'grammar>,
                               lookahead: Lookahead,
                               _conflict: &Conflict<'grammar>)
                               -> Map<LR0Item<'grammar>, LookaheadSet> {
        // Lookahead must be a terminal, not EOF.
        // Find an item J like `Bar = ... (*) L ...`.
        let lookahead = match lookahead {
            Lookahead::EOF => panic!("can't shift EOF"),
            Lookahead::Terminal(s) => Symbol::Terminal(s),
        };
        let mut result = map();
        for item in
            state.items.vec.iter()
                           .filter(|i| i.can_shift())
                           .filter(|i| i.production.symbols[i.index] == lookahead)
        {
            result.entry(item.to_lr0())
                  .or_insert_with(|| LookaheadSet::new(self.grammar))
                  .insert(self.grammar, item.lookahead);
        }
        result
    }
}

//fn choose_example<'grammar>(states: &[State<'grammar>],
//                            lookahead: Lookahead,
//                            conflict: &Conflict<'grammar>)
//{
//    // Whenever we have a conflict in state S, there is always:
//    // - a given lookahead L that permits some reduction, due to
//    //   an item I like `Foo = ... (*) [L]`
//    // - another action that conflicts with R1.
//    //
//    // The backtrace code can give context to this item `I`, but the
//    // problem is that it often results in many different contexts,
//    // and we need to try and narrow those down to the one that will
//    // help the user understand the problem.
//    //
//    // For that, we turn to the conflicting action, which can either be
//    // a shift or reduce. Let's consider those two cases.
//    //
//    // ### Shift
//    //
//    // If the conflicting action is a shift, then there is at least
//    // one item J in the state S like `Bar = ... (*) L ...`. We can
//    // produce a backtrace from J and enumerate examples. We want to
//    // find a pair of examples from I and J that share a common
//    // prefix.
//    //
//    // ### Reduce
//    //
//    // If the conflicting action is a reduce, then there is at least
//    // one item J in S like `Bar = ... (*) [L]`. We can produce a
//    // backtrace for J and then search for an example that shares a
//    // common prefix.
//
//}
//
//fn conflicting_item<'grammar>(state: &State<'grammar>,
//                              lookahead: Lookahead,
//                              conflict: &Conflict<'grammar>)
//                              -> Item<'grammar>
//{
//    match conflict.action {
//        Action::Shift(_) => {
//        }
//        Action::Reduce(production) => {
//            // Must be at least some other item J in S like `Bar = ... (*) [L]`.
//            state.items.vec.iter()
//                           .filter(|i| i.can_reduce())
//                           .filter(|i| i.lookahead == lookahead)
//                           .filter(|i| i.production == production)
//                           .cloned()
//                           .next()
//                           .unwrap()
//        }
//    }
//}

