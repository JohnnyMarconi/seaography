use async_graphql::{
    dynamic::{Field, FieldFuture, FieldValue, InputValue, TypeRef},
    Error,
};
use heck::ToSnakeCase;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, Iden, ModelTrait, QueryFilter, Related,
};

use crate::{
    apply_order, apply_pagination, get_filter_conditions, BuilderContext, ConnectionObjectBuilder,
    EntityObjectBuilder, FilterInputBuilder, GuardAction, OrderInputBuilder,
};

/// This builder produces a GraphQL field for an SeaORM entity related trait
/// that can be added to the entity object
pub struct EntityObjectViaRelationBuilder {
    pub context: &'static BuilderContext,
}

impl EntityObjectViaRelationBuilder {
    /// used to get a GraphQL field for an SeaORM entity related trait
    pub fn get_relation<T, R>(&self, name: &str) -> Field
    where
        T: Related<R>,
        T: EntityTrait,
        R: EntityTrait,
        <T as EntityTrait>::Model: Sync,
        <R as sea_orm::EntityTrait>::Model: Sync,
        <<T as sea_orm::EntityTrait>::Column as std::str::FromStr>::Err: core::fmt::Debug,
        <<R as sea_orm::EntityTrait>::Column as std::str::FromStr>::Err: core::fmt::Debug,
    {
        let context: &'static BuilderContext = self.context;
        let to_relation_definition = <T as Related<R>>::to();
        let (via_relation_definition, is_via_relation) = match <T as Related<R>>::via() {
            Some(def) => (def, true),
            None => (<T as Related<R>>::to(), false),
        };

        let entity_object_builder = EntityObjectBuilder { context };
        let connection_object_builder = ConnectionObjectBuilder { context };
        let filter_input_builder = FilterInputBuilder { context };
        let order_input_builder = OrderInputBuilder { context };

        let object_name: String = entity_object_builder.type_name::<R>();
        let guard = self.context.guards.entity_guards.get(&object_name);

        let from_col = <T::Column as std::str::FromStr>::from_str(
            via_relation_definition
                .from_col
                .to_string()
                .to_snake_case()
                .as_str(),
        )
        .unwrap();

        let to_col = <R::Column as std::str::FromStr>::from_str(
            to_relation_definition
                .to_col
                .to_string()
                .to_snake_case()
                .as_str(),
        )
        .unwrap();

        let field = match via_relation_definition.is_owner {
            false => {
                Field::new(name, TypeRef::named(&object_name), move |ctx| {
                    // FIXME: optimize by adding dataloader
                    FieldFuture::new(async move {
                        let guard_flag = if let Some(guard) = guard {
                            (*guard)(&ctx)
                        } else {
                            GuardAction::Allow
                        };

                        if let GuardAction::Block(reason) = guard_flag {
                            return match reason {
                                Some(reason) => {
                                    Err::<Option<_>, async_graphql::Error>(Error::new(reason))
                                }
                                None => Err::<Option<_>, async_graphql::Error>(Error::new(
                                    "Entity guard triggered.",
                                )),
                            };
                        }

                        let parent: &T::Model = ctx
                            .parent_value
                            .try_downcast_ref::<T::Model>()
                            .expect("Parent should exist");

                        let stmt = if <T as Related<R>>::via().is_some() {
                            <T as Related<R>>::find_related()
                        } else {
                            R::find()
                        };

                        let filter = Condition::all().add(to_col.eq(parent.get(from_col)));

                        let stmt = stmt.filter(filter);

                        let db = ctx.data::<DatabaseConnection>()?;

                        let data = stmt.one(db).await?;

                        if let Some(data) = data {
                            Ok(Some(FieldValue::owned_any(data)))
                        } else {
                            Ok(None)
                        }
                    })
                })
            }
            true => Field::new(
                name,
                TypeRef::named_nn(connection_object_builder.type_name(&object_name)),
                move |ctx| {
                    let context: &'static BuilderContext = context;
                    FieldFuture::new(async move {
                        let guard_flag = if let Some(guard) = guard {
                            (*guard)(&ctx)
                        } else {
                            GuardAction::Allow
                        };

                        if let GuardAction::Block(reason) = guard_flag {
                            return match reason {
                                Some(reason) => {
                                    Err::<Option<_>, async_graphql::Error>(Error::new(reason))
                                }
                                None => Err::<Option<_>, async_graphql::Error>(Error::new(
                                    "Entity guard triggered.",
                                )),
                            };
                        }

                        // FIXME: optimize union queries
                        // NOTE: each has unique query in order to apply pagination...
                        let parent: &T::Model = ctx
                            .parent_value
                            .try_downcast_ref::<T::Model>()
                            .expect("Parent should exist");

                        let stmt = if <T as Related<R>>::via().is_some() {
                            <T as Related<R>>::find_related()
                        } else {
                            R::find()
                        };

                        let condition = if is_via_relation {
                            Condition::all().add(from_col.eq(parent.get(from_col)))
                        } else {
                            Condition::all().add(to_col.eq(parent.get(from_col)))
                        };

                        let filters = ctx.args.get(&context.entity_query_field.filters);
                        let order_by = ctx.args.get(&context.entity_query_field.order_by);
                        let pagination = ctx.args.get(&context.entity_query_field.pagination);

                        let base_condition = get_filter_conditions::<R>(context, filters);

                        let stmt = stmt.filter(condition.add(base_condition));
                        let stmt = apply_order(context, stmt, order_by);

                        let db = ctx.data::<DatabaseConnection>()?;

                        let connection =
                            apply_pagination::<R>(context, db, stmt, pagination).await?;

                        Ok(Some(FieldValue::owned_any(connection)))
                    })
                },
            ),
        };

        match via_relation_definition.is_owner {
            false => field,
            true => field
                .argument(InputValue::new(
                    &context.entity_query_field.filters,
                    TypeRef::named(filter_input_builder.type_name(&object_name)),
                ))
                .argument(InputValue::new(
                    &context.entity_query_field.order_by,
                    TypeRef::named(order_input_builder.type_name(&object_name)),
                ))
                .argument(InputValue::new(
                    &context.entity_query_field.pagination,
                    TypeRef::named(&context.pagination_input.type_name),
                )),
        }
    }
}
