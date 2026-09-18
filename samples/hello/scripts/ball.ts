export class Ball {
    y: number;
    velocity = 0;
    bounces = 0;
    grounded = false;
    readonly gravity = -9.8;
    readonly restitution = 0.7;

    constructor(height: number) {
        this.y = height;
    }

    /** Advance one tick. Returns true when the ball bounced this tick. */
    step(dt: number): boolean {
        this.velocity += this.gravity * dt;
        this.y += this.velocity * dt;
        if (this.y <= 0) {
            this.y = 0;
            if (Math.abs(this.velocity) < 0.3) {
                this.velocity = 0;
                this.grounded = true;
                return false;
            }
            this.velocity = -this.velocity * this.restitution;
            this.bounces++;
            return true;
        }
        this.grounded = false;
        return false;
    }
}
