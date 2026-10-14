//! Dense linear algebra used by the compact SPICE engine.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    pub fn polar(magnitude: f64, phase_rad: f64) -> Self {
        Self {
            re: magnitude * phase_rad.cos(),
            im: magnitude * phase_rad.sin(),
        }
    }

    pub fn zero() -> Self {
        Self::default()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    pub fn scale(self, factor: f64) -> Self {
        Self::new(self.re * factor, self.im * factor)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }

    #[allow(clippy::should_implement_trait)]
    pub fn div(self, other: Self) -> Result<Self, String> {
        let denominator = other.re * other.re + other.im * other.im;
        if !denominator.is_finite() || denominator <= f64::EPSILON {
            return Err("complex division by zero".into());
        }
        Ok(Self::new(
            (self.re * other.re + self.im * other.im) / denominator,
            (self.im * other.re - self.re * other.im) / denominator,
        ))
    }

    pub fn magnitude(self) -> f64 {
        self.re.hypot(self.im)
    }

    pub fn phase_rad(self) -> f64 {
        self.im.atan2(self.re)
    }
}

pub fn solve_real(matrix: Vec<Vec<f64>>, rhs: Vec<f64>) -> Result<Vec<f64>, String> {
    let n = rhs.len();
    if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
        return Err("real matrix/rhs dimension mismatch".into());
    }
    let mut a = matrix;
    let mut b = rhs;
    for row in &a {
        if row.iter().any(|value| !value.is_finite()) || row.iter().all(|value| *value == 0.0) {
            return Err("singular real system".into());
        }
    }
    if b.iter().any(|value| !value.is_finite()) {
        return Err("non-finite real right-hand side".into());
    }

    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&x, &y| {
                a[x][col]
                    .abs()
                    .partial_cmp(&a[y][col].abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or("empty real system")?;
        if !a[pivot][col].is_finite() || a[pivot][col].abs() <= 1e-14 {
            return Err("singular real system".into());
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let diagonal = a[col][col];
        #[allow(clippy::needless_range_loop)]
        for row in (col + 1)..n {
            let factor = a[row][col] / diagonal;
            if factor == 0.0 {
                continue;
            }
            a[row][col] = 0.0;
            #[allow(clippy::needless_range_loop)]
            for k in (col + 1)..n {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }

    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let mut value = b[row];
        for col in (row + 1)..n {
            value -= a[row][col] * x[col];
        }
        x[row] = value / a[row][row];
    }
    if x.iter().any(|value| !value.is_finite()) {
        return Err("non-finite real solution".into());
    }
    Ok(x)
}

pub fn solve_complex(matrix: Vec<Vec<Complex>>, rhs: Vec<Complex>) -> Result<Vec<Complex>, String> {
    let n = rhs.len();
    if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
        return Err("complex matrix/rhs dimension mismatch".into());
    }
    let mut a = matrix;
    let mut b = rhs;
    if a.iter()
        .flatten()
        .any(|value| !value.re.is_finite() || !value.im.is_finite())
        || b.iter()
            .any(|value| !value.re.is_finite() || !value.im.is_finite())
    {
        return Err("non-finite complex system".into());
    }

    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&x, &y| {
                a[x][col]
                    .magnitude()
                    .partial_cmp(&a[y][col].magnitude())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or("empty complex system")?;
        if a[pivot][col].magnitude() <= 1e-16 {
            return Err("singular complex system".into());
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let diagonal = a[col][col];
        #[allow(clippy::needless_range_loop)]
        for row in (col + 1)..n {
            let factor = a[row][col].div(diagonal)?;
            if factor == Complex::zero() {
                continue;
            }
            a[row][col] = Complex::zero();
            #[allow(clippy::needless_range_loop)]
            for k in (col + 1)..n {
                a[row][k] = a[row][k].sub(factor.mul(a[col][k]));
            }
            b[row] = b[row].sub(factor.scale(1.0).mul(b[col]));
        }
    }

    let mut x = vec![Complex::zero(); n];
    for row in (0..n).rev() {
        let mut value = b[row];
        for col in (row + 1)..n {
            value = value.sub(a[row][col].mul(x[col]));
        }
        x[row] = value.div(a[row][row])?;
    }
    Ok(x)
}
