use qbice::storage::intern::Interned;
use rayc_handler::Storage;
use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::{
    GenInferWithSpan,
    path::{Effect, PathResolution, PathResolutionError},
    resolver::Resolver,
};
use rayc_syntax::{path::Path, r#type::Type as TypeSyntax};
use rayc_type::{poly_var::get_enclosing_poly_var_maps, trait_ref::TraitRef, ty::Ty};

use crate::{
    diagnostic::Diagnostic,
    tast_builder::{TAstBuilder, constraint_solver::ResolutionInference},
};

impl TAstBuilder {
    /// Creates a new inference variable for a trait instance, then generates a
    /// constraint that will resolve a concrete instance and assign it to the
    /// inference variable. The inference variable is returned as a
    /// `Ty::Inference` type.
    pub(crate) async fn infer_trait_instance(
        &mut self,
        trait_ref: &TraitRef,
        span: RelativeSpan,
    ) -> Interned<Ty> {
        let mut inference = ResolutionInference::new(&mut self.constraint_solver);
        let instance = inference.gen_instance_infer(trait_ref, span);
        let constraints = inference.into_constraints();
        self.push_constraints(constraints).await;
        self.engine().intern(Ty::Inference(instance))
    }

    pub(crate) async fn resolve_local_type_annotation(
        &mut self,
        syntax: &TypeSyntax,
    ) -> Interned<Ty> {
        let poly_vars = self.engine().get_enclosing_poly_var_maps(self.current_def_id()).await;
        let diagnostics = Storage::new();
        let obligations = Storage::<rayc_resolution::Obligation>::new();
        let mut resolver = Resolver::builder()
            .engine(self.engine())
            .poly_var_stack(&poly_vars)
            .site(self.current_def_id())
            .handler(&diagnostics)
            .obligation_handler(&obligations)
            .build();

        let ty = resolver.resolve_type(syntax).await;

        self.push_resolution_obligations(obligations.into_vec()).await;
        self.extend_diagnostics(diagnostics.into_vec());

        ty
    }

    pub(crate) async fn resolve_effect_path(
        &mut self,
        path: &Path,
    ) -> Result<Effect, PathResolutionError> {
        let poly_vars = self.engine.get_enclosing_poly_var_maps(self.current_def_id).await;
        let diagnostics = Storage::<rayc_resolution::Diagnostic>::new();
        let obligations = Storage::<rayc_resolution::Obligation>::new();
        let mut inference = ResolutionInference::new(&mut self.constraint_solver);

        let resolution = {
            let mut resolver = Resolver::builder()
                .engine(&self.engine)
                .poly_var_stack(&poly_vars)
                .site(self.current_def_id)
                .handler(&diagnostics)
                .obligation_handler(&obligations)
                .infer_gen(&mut inference)
                .build();
            resolver.resolve_effect_path(path).await
        };

        let constraints = inference.into_constraints();
        self.push_constraints(constraints).await;
        self.push_resolution_obligations(obligations.into_vec()).await;
        self.diagnostics
            .extend(diagnostics.into_vec().into_iter().map(crate::diagnostic::Diagnostic::from));
        resolution
    }

    pub(crate) async fn resolve_path(
        &mut self,
        path: &Path,
    ) -> Result<PathResolution, PathResolutionError> {
        let poly_vars = self.engine.get_enclosing_poly_var_maps(self.current_def_id).await;
        let diagnostics = Storage::<rayc_resolution::Diagnostic>::new();
        let obligations = Storage::new();
        let mut inference = ResolutionInference::new(&mut self.constraint_solver);

        let resolution = {
            let mut resolver = Resolver::builder()
                .engine(&self.engine)
                .poly_var_stack(&poly_vars)
                .site(self.current_def_id)
                .handler(&diagnostics)
                .obligation_handler(&obligations)
                .infer_gen(&mut inference)
                .build();
            resolver.resolve_path(path).await
        };

        // Submit even on resolution failure: earlier path segments may have generated
        // obligations.
        let constrs = inference.into_constraints();
        self.push_constraints(constrs).await;
        self.push_resolution_obligations(obligations.into_vec()).await;
        self.diagnostics.extend(diagnostics.into_vec().into_iter().map(Diagnostic::from));
        resolution
    }
}
