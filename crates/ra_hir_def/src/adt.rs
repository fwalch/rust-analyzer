//! Defines hir-level representation of structs, enums and unions

use std::sync::Arc;

use either::Either;
use hir_expand::{
    name::{AsName, Name},
    InFile,
};
use ra_arena::{map::ArenaMap, Arena};
use ra_prof::profile;
use ra_syntax::ast::{self, NameOwner, TypeAscriptionOwner, VisibilityOwner};

use crate::{
    db::DefDatabase, src::HasChildSource, src::HasSource, trace::Trace, type_ref::TypeRef,
    visibility::RawVisibility, EnumId, LocalEnumVariantId, LocalStructFieldId, Lookup, StructId,
    UnionId, VariantId,
};

/// Note that we use `StructData` for unions as well!
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructData {
    pub name: Name,
    pub variant_data: Arc<VariantData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumData {
    pub name: Name,
    pub variants: Arena<EnumVariantData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariantData {
    pub name: Name,
    pub variant_data: Arc<VariantData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantData {
    Record(Arena<StructFieldData>),
    Tuple(Arena<StructFieldData>),
    Unit,
}

/// A single field of an enum variant or struct
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructFieldData {
    pub name: Name,
    pub type_ref: TypeRef,
    pub visibility: RawVisibility,
}

impl StructData {
    pub(crate) fn struct_data_query(db: &dyn DefDatabase, id: StructId) -> Arc<StructData> {
        let src = id.lookup(db).source(db);

        let name = src.value.name().map_or_else(Name::missing, |n| n.as_name());
        let variant_data = VariantData::new(db, src.map(|s| s.kind()));
        let variant_data = Arc::new(variant_data);
        Arc::new(StructData { name, variant_data })
    }
    pub(crate) fn union_data_query(db: &dyn DefDatabase, id: UnionId) -> Arc<StructData> {
        let src = id.lookup(db).source(db);
        let name = src.value.name().map_or_else(Name::missing, |n| n.as_name());
        let variant_data = VariantData::new(
            db,
            src.map(|s| {
                s.record_field_def_list()
                    .map(ast::StructKind::Record)
                    .unwrap_or(ast::StructKind::Unit)
            }),
        );
        let variant_data = Arc::new(variant_data);
        Arc::new(StructData { name, variant_data })
    }
}

impl EnumData {
    pub(crate) fn enum_data_query(db: &dyn DefDatabase, e: EnumId) -> Arc<EnumData> {
        let _p = profile("enum_data_query");
        let src = e.lookup(db).source(db);
        let name = src.value.name().map_or_else(Name::missing, |n| n.as_name());
        let mut trace = Trace::new_for_arena();
        lower_enum(db, &mut trace, &src);
        Arc::new(EnumData { name, variants: trace.into_arena() })
    }

    pub fn variant(&self, name: &Name) -> Option<LocalEnumVariantId> {
        let (id, _) = self.variants.iter().find(|(_id, data)| &data.name == name)?;
        Some(id)
    }
}

impl HasChildSource for EnumId {
    type ChildId = LocalEnumVariantId;
    type Value = ast::EnumVariant;
    fn child_source(&self, db: &dyn DefDatabase) -> InFile<ArenaMap<Self::ChildId, Self::Value>> {
        let src = self.lookup(db).source(db);
        let mut trace = Trace::new_for_map();
        lower_enum(db, &mut trace, &src);
        src.with_value(trace.into_map())
    }
}

fn lower_enum(
    db: &dyn DefDatabase,
    trace: &mut Trace<EnumVariantData, ast::EnumVariant>,
    ast: &InFile<ast::EnumDef>,
) {
    for var in ast.value.variant_list().into_iter().flat_map(|it| it.variants()) {
        trace.alloc(
            || var.clone(),
            || EnumVariantData {
                name: var.name().map_or_else(Name::missing, |it| it.as_name()),
                variant_data: Arc::new(VariantData::new(db, ast.with_value(var.kind()))),
            },
        );
    }
}

impl VariantData {
    fn new(db: &dyn DefDatabase, flavor: InFile<ast::StructKind>) -> Self {
        let mut trace = Trace::new_for_arena();
        match lower_struct(db, &mut trace, &flavor) {
            StructKind::Tuple => VariantData::Tuple(trace.into_arena()),
            StructKind::Record => VariantData::Record(trace.into_arena()),
            StructKind::Unit => VariantData::Unit,
        }
    }

    pub fn fields(&self) -> &Arena<StructFieldData> {
        const EMPTY: &Arena<StructFieldData> = &Arena::new();
        match &self {
            VariantData::Record(fields) | VariantData::Tuple(fields) => fields,
            _ => EMPTY,
        }
    }

    pub fn field(&self, name: &Name) -> Option<LocalStructFieldId> {
        self.fields().iter().find_map(|(id, data)| if &data.name == name { Some(id) } else { None })
    }

    pub fn kind(&self) -> StructKind {
        match self {
            VariantData::Record(_) => StructKind::Record,
            VariantData::Tuple(_) => StructKind::Tuple,
            VariantData::Unit => StructKind::Unit,
        }
    }
}

impl HasChildSource for VariantId {
    type ChildId = LocalStructFieldId;
    type Value = Either<ast::TupleFieldDef, ast::RecordFieldDef>;

    fn child_source(&self, db: &dyn DefDatabase) -> InFile<ArenaMap<Self::ChildId, Self::Value>> {
        let src = match self {
            VariantId::EnumVariantId(it) => {
                // I don't really like the fact that we call into parent source
                // here, this might add to more queries then necessary.
                let src = it.parent.child_source(db);
                src.map(|map| map[it.local_id].kind())
            }
            VariantId::StructId(it) => it.lookup(db).source(db).map(|it| it.kind()),
            VariantId::UnionId(it) => it.lookup(db).source(db).map(|it| {
                it.record_field_def_list()
                    .map(ast::StructKind::Record)
                    .unwrap_or(ast::StructKind::Unit)
            }),
        };
        let mut trace = Trace::new_for_map();
        lower_struct(db, &mut trace, &src);
        src.with_value(trace.into_map())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StructKind {
    Tuple,
    Record,
    Unit,
}

fn lower_struct(
    db: &dyn DefDatabase,
    trace: &mut Trace<StructFieldData, Either<ast::TupleFieldDef, ast::RecordFieldDef>>,
    ast: &InFile<ast::StructKind>,
) -> StructKind {
    match &ast.value {
        ast::StructKind::Tuple(fl) => {
            for (i, fd) in fl.fields().enumerate() {
                trace.alloc(
                    || Either::Left(fd.clone()),
                    || StructFieldData {
                        name: Name::new_tuple_field(i),
                        type_ref: TypeRef::from_ast_opt(fd.type_ref()),
                        visibility: RawVisibility::from_ast(db, ast.with_value(fd.visibility())),
                    },
                );
            }
            StructKind::Tuple
        }
        ast::StructKind::Record(fl) => {
            for fd in fl.fields() {
                trace.alloc(
                    || Either::Right(fd.clone()),
                    || StructFieldData {
                        name: fd.name().map(|n| n.as_name()).unwrap_or_else(Name::missing),
                        type_ref: TypeRef::from_ast_opt(fd.ascribed_type()),
                        visibility: RawVisibility::from_ast(db, ast.with_value(fd.visibility())),
                    },
                );
            }
            StructKind::Record
        }
        ast::StructKind::Unit => StructKind::Unit,
    }
}
