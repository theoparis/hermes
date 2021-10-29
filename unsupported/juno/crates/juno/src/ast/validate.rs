/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use super::{
    kind::*, AssignmentExpressionOperator, BinaryExpressionOperator, Context, ExportKind,
    GCContext, ImportKind, LogicalExpressionOperator, MethodDefinitionKind, Node, NodeLabel,
    NodeList, NodePtr, NodeString, NodeVariant, PropertyKind, UnaryExpressionOperator,
    UpdateExpressionOperator, VariableDeclarationKind, Visitor,
};

macro_rules! gen_validate_fn {
    ($name:ident {
        $(
            $kind:ident $([ $parent:ident ])? $({
                $(
                    $field:ident : $type:ty
                    $( [ $( $constraint:ident ),* ] )?
                ),*
                $(,)?
            })?
        ),*
        $(,)?
    }) => {
            /// Check whether this is a valid kind for `node`.
            fn validate_node<'gc>(
                ctx: &'gc GCContext,
                node: &'gc Node<'gc>,
            ) -> Result<(), ValidationError> {
                match node {
                    $(
                        Node::$kind($kind {$($($field,)*)? .. }) => {
                            // Run the validation for each child.
                            // Use `true &&` to make it work when there's no children.
                            $($(
                                $field.validate_child(ctx, node, &[$($(NodeVariant::$constraint),*)?])?;
                            )*)?
                        }
                    ),*
                };
                validate_custom(ctx, node)?;
                Ok(())
            }
    }
}

nodekind_defs! { gen_validate_fn }

trait ValidChild<'gc> {
    /// Check whether this is a valid child of `node` given the constraints.
    fn validate_child(
        &self,
        _ctx: &'gc GCContext,
        _node: &'gc Node<'gc>,
        _constraints: &[NodeVariant],
    ) -> Result<(), ValidationError> {
        Ok(())
    }
}

impl ValidChild<'_> for f64 {}
impl ValidChild<'_> for bool {}
impl ValidChild<'_> for NodeLabel {}
impl ValidChild<'_> for UnaryExpressionOperator {}
impl ValidChild<'_> for BinaryExpressionOperator {}
impl ValidChild<'_> for LogicalExpressionOperator {}
impl ValidChild<'_> for UpdateExpressionOperator {}
impl ValidChild<'_> for AssignmentExpressionOperator {}
impl ValidChild<'_> for VariableDeclarationKind {}
impl ValidChild<'_> for PropertyKind {}
impl ValidChild<'_> for MethodDefinitionKind {}
impl ValidChild<'_> for ImportKind {}
impl ValidChild<'_> for ExportKind {}
impl ValidChild<'_> for NodeString {}

impl<'gc, T: ValidChild<'gc>> ValidChild<'gc> for Option<T> {
    fn validate_child(
        &self,
        ctx: &'gc GCContext,
        node: &'gc Node<'gc>,
        constraints: &[NodeVariant],
    ) -> Result<(), ValidationError> {
        match self {
            None => Ok(()),
            Some(t) => t.validate_child(ctx, node, constraints),
        }
    }
}

impl<'gc> ValidChild<'gc> for &Node<'gc> {
    fn validate_child(
        &self,
        ctx: &'gc GCContext,
        node: &'gc Node<'gc>,
        constraints: &[NodeVariant],
    ) -> Result<(), ValidationError> {
        for &constraint in constraints {
            if instanceof(self.variant(), constraint) {
                return Ok(());
            }
        }
        Err(ValidationError::new(
            ctx,
            node,
            format!("Unexpected {:?}", self.variant()),
        ))
    }
}

impl<'gc> ValidChild<'gc> for NodeList<'gc> {
    fn validate_child(
        &self,
        ctx: &'gc GCContext,
        node: &'gc Node<'gc>,
        constraints: &[NodeVariant],
    ) -> Result<(), ValidationError> {
        'elems: for elem in self {
            for &constraint in constraints {
                if instanceof(elem.variant(), constraint) {
                    // Found a valid constraint for this element,
                    // move on to the next element.
                    continue 'elems;
                }
            }
            // Failed to find a constraint that matched, early return.
            return Err(ValidationError::new(
                ctx,
                node,
                format!("Unexpected {:?}", elem.variant()),
            ));
        }
        Ok(())
    }
}

/// Return whether `subtype` contains `supertype` in its parent chain.
fn instanceof(subtype: NodeVariant, supertype: NodeVariant) -> bool {
    let mut cur = subtype;
    loop {
        if cur == supertype {
            return true;
        }
        match cur.parent() {
            None => return false,
            Some(next) => {
                cur = next;
            }
        }
    }
}

/// Custom validation function for constraints which can't be expressed
/// using just the inheritance structure in Node.
fn validate_custom<'gc>(ctx: &'gc GCContext, node: &'gc Node<'gc>) -> Result<(), ValidationError> {
    match node {
        Node::MemberExpression(MemberExpression {
            metadata: _,
            property,
            object: _,
            computed,
        })
        | Node::OptionalMemberExpression(OptionalMemberExpression {
            metadata: _,
            property,
            object: _,
            computed,
            optional: _,
        }) => {
            if *computed {
                property.validate_child(ctx, node, &[NodeVariant::Expression])?;
            } else {
                property.validate_child(ctx, node, &[NodeVariant::Identifier])?;
            }
        }

        Node::Property(Property {
            metadata: _,
            key,
            value,
            kind,
            computed,
            method,
            shorthand,
        }) => {
            if *computed && *shorthand {
                return Err(ValidationError::new(
                    ctx,
                    node,
                    "Property cannot be computed and shorthand".to_string(),
                ));
            }
            if !*computed {
                key.validate_child(ctx, node, &[NodeVariant::Identifier, NodeVariant::Literal])?;
            }
            if *method {
                value.validate_child(ctx, node, &[NodeVariant::FunctionExpression])?;
            }
            if *kind == PropertyKind::Get || *kind == PropertyKind::Set {
                value.validate_child(ctx, node, &[NodeVariant::FunctionExpression])?;
            }
        }

        _ => {}
    }
    Ok(())
}

/// An AST validation error.
pub struct ValidationError {
    /// The AST node which failed to validate.
    pub node: NodePtr,

    /// A description of the invalid state encountered.
    pub message: String,
}

impl ValidationError {
    pub fn new<'gc>(gc: &'gc GCContext, node: &'gc Node<'gc>, message: String) -> ValidationError {
        ValidationError {
            node: NodePtr::from_node(gc, node),
            message,
        }
    }
}

/// Runs validation on the AST and stores errors.
struct Validator {
    /// Every error encountered so far.
    /// If empty after validation, the AST is valid.
    pub errors: Vec<ValidationError>,
}

impl Validator {
    pub fn new() -> Self {
        Validator { errors: Vec::new() }
    }

    /// Run validation recursively starting at the `root`.
    pub fn validate_root<'gc>(&mut self, ctx: &'gc GCContext, root: &'gc Node<'gc>) {
        self.validate_node(ctx, root);
    }

    /// Validate `node` and recursively validate its children.
    fn validate_node<'gc>(&mut self, ctx: &'gc GCContext, node: &'gc Node<'gc>) {
        if let Err(e) = validate_node(ctx, node) {
            self.errors.push(e);
        }
        node.visit_children(ctx, self);
    }
}

impl<'gc> Visitor<'gc> for Validator {
    fn call(&mut self, ctx: &'gc GCContext, node: &'gc Node<'gc>, _parent: Option<&'gc Node<'gc>>) {
        self.validate_node(ctx, node);
    }
}

/// Validate the full AST tree.
/// If it fails, return all the errors encountered along the way.
pub fn validate_tree(ctx: &mut Context, root: &NodePtr) -> Result<(), Vec<ValidationError>> {
    let mut validator = Validator::new();
    let gc = GCContext::new(ctx);
    validator.validate_root(&gc, root.node(&gc));
    if validator.errors.is_empty() {
        Ok(())
    } else {
        Err(validator.errors)
    }
}
