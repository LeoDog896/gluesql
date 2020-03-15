use crate::data::Row;
use crate::executor::{
    fetch_columns, Blend, BlendContext, BlendedFilter, Filter, FilterContext, Limit,
};
use crate::storage::Store;
use nom_sql::{JoinClause, JoinConstraint, JoinOperator, JoinRightSide, SelectStatement, Table};
use std::fmt::Debug;
use std::rc::Rc;

fn fetch_blended<'a, T: 'static + Debug>(
    storage: &dyn Store<T>,
    table: &'a Table,
) -> Box<dyn Iterator<Item = BlendContext<'a, T>> + 'a> {
    let columns = Rc::new(fetch_columns(storage, table));

    let rows = storage
        .get_data(&table.name)
        .unwrap()
        .map(move |(key, row)| (Rc::clone(&columns), key, row))
        .map(move |(columns, key, row)| BlendContext {
            table,
            columns,
            key,
            row,
            next: None,
        });

    Box::new(rows)
}

pub fn join<'a, T: 'static + Debug>(
    storage: &'a dyn Store<T>,
    join_clause: &'a JoinClause,
    filter_context: Option<&'a FilterContext<'a>>,
    blend_context: BlendContext<'a, T>,
) -> Option<BlendContext<'a, T>> {
    let JoinClause {
        operator,
        right,
        constraint,
    } = join_clause;

    let table = match right {
        JoinRightSide::Table(table) => table,
        _ => unimplemented!(),
    };
    let where_clause = match constraint {
        JoinConstraint::On(where_clause) => Some(where_clause),
        _ => unimplemented!(),
    };
    let filter = Filter::new(storage, where_clause, filter_context);
    let blended_filter = BlendedFilter::new(&filter, &blend_context);
    let columns = Rc::new(fetch_columns(storage, table));

    let row = storage
        .get_data(&table.name)
        .unwrap()
        .map(move |(key, row)| (Rc::clone(&columns), key, row))
        .filter(move |(columns, _, row)| blended_filter.check(table, columns, row))
        .nth(0);

    match row {
        Some((columns, key, row)) => Some(BlendContext {
            table,
            columns,
            key,
            row,
            next: Some(Box::new(blend_context)),
        }),
        None => match operator {
            JoinOperator::LeftJoin | JoinOperator::LeftOuterJoin => Some(blend_context),
            JoinOperator::Join | JoinOperator::InnerJoin => None,
            _ => unimplemented!(),
        },
    }
}

pub fn select<'a, T: 'static + Debug>(
    storage: &'a dyn Store<T>,
    statement: &'a SelectStatement,
    filter_context: Option<&'a FilterContext<'a>>,
) -> Box<dyn Iterator<Item = Row> + 'a> {
    let SelectStatement {
        tables,
        where_clause,
        limit: limit_clause,
        join: join_clauses,
        fields,
        ..
    } = statement;
    let table = &tables
        .iter()
        .nth(0)
        .expect("SelectStatement->tables should have something");
    let blend = Blend::new(fields);
    let filter = Filter::new(storage, where_clause.as_ref(), filter_context);
    let limit = Limit::new(limit_clause);

    let rows = fetch_blended(storage, table)
        .filter_map(move |init_context| {
            join_clauses
                .iter()
                .fold(Some(init_context), |blend_context, join_clause| {
                    blend_context.and_then(|blend_context| {
                        join(storage, join_clause, filter_context, blend_context)
                    })
                })
        })
        .filter(move |blend_context| {
            let BlendContext {
                table,
                columns,
                row,
                ..
            } = blend_context;
            let blended_filter = BlendedFilter::new(&filter, &blend_context);

            // TODO: It's certainly redundant!
            blended_filter.check(&table, &columns, &row)
        })
        .enumerate()
        .filter_map(move |(i, item)| match limit.check(i) {
            true => Some(item),
            false => None,
        })
        .map(move |BlendContext { columns, row, .. }| blend.apply(&columns, row));

    Box::new(rows)
}