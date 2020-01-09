use crate::misc_helpers::*;
use crate::sanitize_datamodel_names::sanitize_datamodel_names;
use crate::SqlIntrospectionResult;
use datamodel::{dml, Datamodel, FieldType, Model};
use log::debug;
use sql_schema_describer::*;

/// Calculate a data model from a database schema.
pub fn calculate_model(schema: &SqlSchema) -> SqlIntrospectionResult<Datamodel> {
    debug!("Calculating data model");

    let mut data_model = Datamodel::new();
    for table in schema
        .tables
        .iter()
        .filter(|table| !is_migration_table(&table))
        .filter(|table| !is_prisma_join_table(&table))
    {
        let mut model = Model::new(table.name.clone(), None);

        for column in table
            .columns
            .iter()
            .filter(|column| !is_compound_foreign_key_column(&table, &column))
        {
            let field = calculate_non_compound_field(&schema, &table, &column);
            model.add_field(field);
        }

        //do not add compound indexes to schema when they cover a foreign key, instead make the relation 1:1
        for index in table.indices.iter().filter(|i| {
            table
                .foreign_keys
                .iter()
                .all(|fk| !is_foreign_key_covered_by_unique_index(i, fk))
        }) {
            match (index.columns.len(), &index.tpe) {
                (1, IndexType::Unique) => (), // they go on the field not the model in the datamodel
                _ => model.add_index(calculate_index(index)),
            }
        }

        //add compound fields
        for foreign_key in table.foreign_keys.iter().filter(|fk| fk.columns.len() > 1) {
            let field = calculate_compound_field(schema, table, foreign_key);
            model.add_field(field);
        }

        if table.primary_key_columns().len() > 1 {
            model.id_fields = table.primary_key_columns();
        }

        data_model.add_model(model);
    }

    for e in schema.enums.iter() {
        let mut values: Vec<String> = e.values.iter().cloned().collect();
        values.sort_unstable();
        data_model.add_enum(dml::Enum {
            name: e.name.clone(),
            values,
            database_name: None,
            documentation: None,
        });
    }

    let mut fields_to_be_added = Vec::new();

    // add backrelation fields
    for model in data_model.models.iter() {
        for relation_field in model.fields.iter() {
            if let FieldType::Relation(relation_info) = &relation_field.field_type {
                if data_model
                    .related_field(
                        &model.name,
                        &relation_info.to,
                        &relation_info.name,
                        &relation_field.name,
                    )
                    .is_none()
                {
                    let other_model = data_model.find_model(relation_info.to.as_str()).unwrap();
                    let field = calculate_backrelation_field(schema, &model, &relation_field, relation_info);

                    fields_to_be_added.push((other_model.name.clone(), field));
                }
            }
        }
    }

    // add prisma many to many relation fields
    for table in schema.tables.iter().filter(|table| is_prisma_join_table(&table)) {
        if let (Some(f), Some(s)) = (table.foreign_keys.get(0), table.foreign_keys.get(1)) {
            let is_self_relation = f.referenced_table == s.referenced_table;

            fields_to_be_added.push((
                s.referenced_table.clone(),
                calculate_many_to_many_field(f, table.name[1..].to_string(), is_self_relation),
            ));
            fields_to_be_added.push((
                f.referenced_table.clone(),
                calculate_many_to_many_field(s, table.name[1..].to_string(), is_self_relation),
            ));
        }
    }

    deduplicate_names_of_fields_to_be_added(&mut fields_to_be_added);

    for (model, field) in fields_to_be_added {
        let model = data_model.find_model_mut(&model).unwrap();
        model.add_field(field);
    }

    Ok(sanitize_datamodel_names(data_model))
}
