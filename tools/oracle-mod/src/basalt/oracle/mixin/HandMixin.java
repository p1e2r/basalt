package basalt.oracle.mixin;

import basalt.oracle.Anim;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.ItemInHandRenderer;
import net.minecraft.client.renderer.SubmitNodeCollector;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.item.ItemDisplayContext;
import net.minecraft.world.item.ItemStack;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Test-only: records the pose at which a first-person held item is drawn (Anim "fp" recorder). */
@Mixin(ItemInHandRenderer.class)
public abstract class HandMixin {
    @Inject(method = "renderItem", at = @At("HEAD"))
    private void basalt$item(LivingEntity e, ItemStack s, ItemDisplayContext ctx, PoseStack ps, SubmitNodeCollector c, int light, CallbackInfo ci) {
        if (ctx == ItemDisplayContext.FIRST_PERSON_RIGHT_HAND || ctx == ItemDisplayContext.FIRST_PERSON_LEFT_HAND) Anim.fpRecord("item", ps, ctx.name());
    }
}
